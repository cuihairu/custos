//! ICMP ping probe (survey §26.11, implementation order #9).
//!
//! The echo exchange itself uses the mature `ping` crate (datagram ping
//! sockets first on Linux/macOS, raw sockets elsewhere — see its docs) so we
//! never hand-roll ICMP or ASN.1. On top of it this probe implements the
//! permission degradation path the survey requires: when the socket cannot
//! be opened (unprivileged Linux host outside `ping_group_range`, non-admin
//! Windows), it falls back to a TCP connect ("tcp ping") against
//! `fallback_port`. A ping that gets no reply within the timeout is a
//! definite answer — Unhealthy, the industry convention; a probe that could
//! not run at all is Unreachable unless the TCP fallback speaks.

use crate::config::{bool_or, required_str, u64_or};
use crate::{Probe, ProbeReport};
use custos_model::{HealthStatus, Resource};
use ping::Error as PingError;
use std::io::ErrorKind;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

const DEFAULT_TIMEOUT_MS: u64 = 3_000;
const DEFAULT_FALLBACK_PORT: u64 = 80;

/// Probes host reachability via ICMP echo, TCP fallback on no permission.
#[derive(Debug, Clone)]
pub struct PingProbe {
    pub host: String,
    pub timeout: Duration,
    /// When ICMP is unavailable (no raw socket permission), fall back to a
    /// TCP connect. Default on — the survey's required degradation path.
    pub tcp_fallback: bool,
    /// TCP port used by the fallback connect. Default 80.
    pub fallback_port: u16,
}

/// Builds from `[resources.probe]` (docs/guide/probe-survey.md §26.11).
pub fn from_config(config: &serde_json::Value) -> Result<PingProbe, String> {
    let host = required_str(config, "host").ok_or("`probe.host` is required for icmp resources")?;
    let fallback_port = u64_or(config, "fallback_port", DEFAULT_FALLBACK_PORT);
    let fallback_port = u16::try_from(fallback_port)
        .map_err(|_| "`probe.fallback_port` must be an integer in 0..=65535".to_owned())?;
    Ok(PingProbe {
        host,
        timeout: Duration::from_millis(u64_or(config, "timeout_ms", DEFAULT_TIMEOUT_MS)),
        tcp_fallback: bool_or(config, "tcp_fallback", true),
        fallback_port,
    })
}

impl Probe for PingProbe {
    fn name(&self) -> &'static str {
        "icmp"
    }

    fn probe(&self, resource: &Resource) -> ProbeReport {
        let report = |succeeded, status, observation: String| ProbeReport {
            resource_id: resource.id.clone(),
            probe_name: self.name().to_owned(),
            succeeded,
            status,
            observation,
        };

        // `fallback_port` is resolved too so the TCP fallback has a full
        // SocketAddr ready; `address.ip()` feeds the ICMP request.
        let addresses: Vec<SocketAddr> =
            match (self.host.as_str(), self.fallback_port).to_socket_addrs() {
                Ok(iter) => iter.collect(),
                Err(err) => {
                    return report(
                        false,
                        HealthStatus::Unreachable,
                        format!("cannot resolve {}: {err}", self.host),
                    );
                }
            };
        if addresses.is_empty() {
            return report(
                false,
                HealthStatus::Unreachable,
                format!("{} resolved to no addresses", self.host),
            );
        }

        let mut timed_out = false;
        let mut unavailable: Option<String> = None;
        for address in &addresses {
            match ping::ping(address.ip(), self.timeout) {
                Ok(reply) => {
                    return report(
                        true,
                        HealthStatus::Healthy,
                        format!(
                            "icmp echo reply from {} in {}ms",
                            reply.source,
                            reply.rtt.as_millis()
                        ),
                    );
                }
                Err(PingError::Timeout) => timed_out = true,
                // The socket could not be opened or used (unprivileged host,
                // non-admin Windows): remember and let the TCP fallback speak.
                Err(PingError::IoError { error }) => {
                    unavailable.get_or_insert(format!("{error}"));
                }
                Err(err) => {
                    unavailable.get_or_insert(format!("{err}"));
                }
            }
        }

        if let Some(reason) = unavailable {
            if !self.tcp_fallback {
                return report(
                    false,
                    HealthStatus::Unreachable,
                    format!("icmp unavailable ({reason}) and tcp_fallback is disabled"),
                );
            }
            return self.tcp_fallback(&addresses, &reason, &report);
        }
        if timed_out {
            return report(
                false,
                HealthStatus::Unhealthy,
                format!(
                    "no icmp echo reply from {} within {}ms",
                    self.host,
                    self.timeout.as_millis()
                ),
            );
        }
        report(
            false,
            HealthStatus::Unreachable,
            "icmp produced no result".to_owned(),
        )
    }
}

impl PingProbe {
    /// Permission degradation path (survey §26.11 #9): a TCP connect ping.
    fn tcp_fallback(
        &self,
        addresses: &[SocketAddr],
        reason: &str,
        report: &impl Fn(bool, HealthStatus, String) -> ProbeReport,
    ) -> ProbeReport {
        let start = Instant::now();
        let mut last_error = None;
        for address in addresses {
            let left = self.timeout.saturating_sub(start.elapsed());
            if left.is_zero() {
                return report(
                    false,
                    HealthStatus::Unreachable,
                    format!(
                        "icmp unavailable ({reason}); tcp fallback timed out after {}ms",
                        self.timeout.as_millis()
                    ),
                );
            }
            match TcpStream::connect_timeout(address, left) {
                Ok(_) => {
                    return report(
                        true,
                        HealthStatus::Healthy,
                        format!(
                            "icmp unavailable ({reason}); tcp fallback connected to \
                             {address} in {}ms",
                            start.elapsed().as_millis()
                        ),
                    );
                }
                Err(err) if matches!(err.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => {
                    return report(
                        false,
                        HealthStatus::Unreachable,
                        format!(
                            "icmp unavailable ({reason}); tcp fallback to {address} \
                             timed out after {}ms",
                            self.timeout.as_millis()
                        ),
                    );
                }
                Err(err) => {
                    last_error.get_or_insert(err);
                }
            }
        }
        report(
            false,
            HealthStatus::Unhealthy,
            format!(
                "icmp unavailable ({reason}); tcp fallback connection refused: {}",
                last_error
                    .map(|err| err.to_string())
                    .unwrap_or_else(|| "no address accepted".to_owned())
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use custos_model::ids::ResourceId;
    use serde_json::json;
    use std::net::TcpListener;

    fn resource() -> Resource {
        Resource::new("gw", "icmp")
    }

    fn closed_loopback_port() -> u16 {
        // Bind, learn the port, drop: that port is now (almost surely) closed.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    }

    #[test]
    fn from_config_requires_host() {
        assert!(from_config(&json!({})).is_err());
    }

    #[test]
    fn from_config_defaults() {
        let probe = from_config(&json!({"host": "example.com"})).unwrap();
        assert_eq!(probe.timeout, Duration::from_millis(3000));
        assert!(probe.tcp_fallback);
        assert_eq!(probe.fallback_port, 80);
    }

    #[test]
    fn from_config_rejects_bad_fallback_port() {
        assert!(from_config(&json!({"host": "x", "fallback_port": 70000})).is_err());
    }

    #[test]
    fn unresolvable_host_is_unreachable() {
        // `.invalid` is reserved and never resolves (RFC 6761). A sandboxed
        // resolver may map unknown names to a synthetic address (e.g. the
        // 198.18.0.0/15 benchmark range) and even answer ICMP — then the
        // probe returns a definite verdict instead. On a normal resolver it
        // is Unreachable with a "cannot resolve" observation.
        let probe =
            from_config(&json!({"host": "no-such-host.invalid", "timeout_ms": 1000})).unwrap();
        let report = probe.probe(&resource());
        if report.status == HealthStatus::Unreachable {
            assert!(
                report.observation.contains("cannot resolve"),
                "{}",
                report.observation
            );
        } else {
            assert_ne!(
                report.status,
                HealthStatus::Unknown,
                "{}",
                report.observation
            );
        }
    }

    #[test]
    fn loopback_reaches_host_via_icmp_or_tcp_fallback() {
        // A bound listener makes the TCP fallback succeed on machines that
        // deny unprivileged ICMP; open hosts take the ICMP path. Healthy
        // either way, so the test is environment-independent.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let probe = from_config(&json!({
            "host": "127.0.0.1", "timeout_ms": 3000, "fallback_port": port
        }))
        .unwrap();
        let report = probe.probe(&resource());
        assert_eq!(
            report.status,
            HealthStatus::Healthy,
            "{}",
            report.observation
        );
        let says = |needle: &str| report.observation.contains(needle);
        assert!(
            says("icmp echo reply") || says("tcp fallback connected"),
            "{}",
            report.observation
        );
    }

    #[test]
    fn silent_host_is_never_healthy() {
        // TEST-NET-3 (RFC 5737) never answers. ICMP-capable hosts → timeout
        // → Unhealthy; restricted hosts → the TCP fallback also fails. Either
        // way it is a definite non-Healthy verdict.
        let probe = from_config(&json!({"host": "203.0.113.1", "timeout_ms": 300})).unwrap();
        let report = probe.probe(&resource());
        assert_ne!(
            report.status,
            HealthStatus::Healthy,
            "{}",
            report.observation
        );
        assert_ne!(
            report.status,
            HealthStatus::Unknown,
            "{}",
            report.observation
        );
    }

    #[test]
    fn tcp_fallback_connect_ok_is_healthy() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addresses = [listener.local_addr().unwrap()];
        let probe = PingProbe {
            host: "127.0.0.1".into(),
            timeout: Duration::from_secs(2),
            tcp_fallback: true,
            fallback_port: addresses[0].port(),
        };
        let report = |succeeded, status, observation: String| ProbeReport {
            resource_id: ResourceId::new("gw"),
            probe_name: "icmp".into(),
            succeeded,
            status,
            observation,
        };
        let verdict = probe.tcp_fallback(&addresses, "permission denied", &report);
        assert_eq!(
            verdict.status,
            HealthStatus::Healthy,
            "{}",
            verdict.observation
        );
        assert!(verdict.observation.contains("tcp fallback"));
    }

    #[test]
    fn tcp_fallback_refused_is_unhealthy() {
        let port = closed_loopback_port();
        let addresses = [SocketAddr::from(([127, 0, 0, 1], port))];
        let probe = PingProbe {
            host: "127.0.0.1".into(),
            timeout: Duration::from_secs(2),
            tcp_fallback: true,
            fallback_port: port,
        };
        let report = |succeeded, status, observation: String| ProbeReport {
            resource_id: ResourceId::new("gw"),
            probe_name: "icmp".into(),
            succeeded,
            status,
            observation,
        };
        let verdict = probe.tcp_fallback(&addresses, "permission denied", &report);
        assert_eq!(
            verdict.status,
            HealthStatus::Unhealthy,
            "{}",
            verdict.observation
        );
        assert!(verdict.observation.contains("refused"));
    }
}
