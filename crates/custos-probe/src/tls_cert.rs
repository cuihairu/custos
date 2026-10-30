//! TLS certificate expiry probe (survey §26.8, implementation order #4).
//!
//! Connects to an arbitrary `host:port`, completes a TLS handshake with an
//! accept-any verifier (so *expired* or *not-yet-valid* certificates still
//! complete — the validity dates need to be read, not rejected), then reads
//! the peer's end-entity certificate and classifies the remaining validity:
//! Healthy above `warn_days`, Degraded inside `warn_days`, Unhealthy inside
//! `crit_days` or already expired. No HTTP request is made — the probe works
//! for any TLS endpoint, not just HTTPS.

use crate::config::{optional_str, required_str, u64_or};
use crate::{tls, Probe, ProbeReport};
use custos_model::{HealthStatus, Resource};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};
use x509_parser::parse_x509_certificate;

const DEFAULT_PORT: u16 = 443;
const DEFAULT_TIMEOUT_MS: u64 = 5_000;
const DEFAULT_WARN_DAYS: u64 = 30;
const DEFAULT_CRIT_DAYS: u64 = 7;
const SECS_PER_DAY: i64 = 86_400;

/// Probes the TLS certificate expiry of one host:port.
#[derive(Debug, Clone)]
pub struct TlsCertProbe {
    pub host: String,
    pub port: u16,
    /// SNI / verification name; defaults to `host` (lets an IP literal be
    /// probed against the domain name the cert was issued for).
    pub server_name: String,
    pub timeout: Duration,
    pub warn_days: u64,
    pub crit_days: u64,
}

/// Builds from `[resources.probe]` (docs/guide/probe-survey.md §26.8).
pub fn from_config(config: &serde_json::Value) -> Result<TlsCertProbe, String> {
    let host =
        required_str(config, "host").ok_or("`probe.host` is required for tls-certificate")?;
    if host.trim().is_empty() {
        return Err("`probe.host` must not be empty".to_owned());
    }
    let port = match config["port"].as_u64() {
        Some(port) => u16::try_from(port)
            .map_err(|_| "`probe.port` must be an integer in 0..=65535".to_owned())?,
        None => DEFAULT_PORT,
    };
    let warn_days = u64_or(config, "warn_days", DEFAULT_WARN_DAYS);
    let crit_days = u64_or(config, "crit_days", DEFAULT_CRIT_DAYS);
    if crit_days > warn_days {
        return Err(format!(
            "`probe.crit_days` ({crit_days}) must be <= `probe.warn_days` ({warn_days})"
        ));
    }
    Ok(TlsCertProbe {
        server_name: optional_str(config, "server_name").unwrap_or_else(|| host.clone()),
        host,
        port,
        timeout: Duration::from_millis(u64_or(config, "timeout_ms", DEFAULT_TIMEOUT_MS)),
        warn_days,
        crit_days,
    })
}

/// Remaining whole days until `not_after`, negative once expired (ceil for
/// the "expired N days ago" side, floor otherwise).
fn days_remaining(not_after_secs: i64, now_secs: i64) -> i64 {
    let secs = not_after_secs - now_secs;
    if secs >= 0 {
        secs / SECS_PER_DAY
    } else {
        -((-secs + SECS_PER_DAY - 1) / SECS_PER_DAY)
    }
}

/// Maps remaining validity to a (status, observation) pair by the day
/// thresholds. `crit_days` is the harder limit and must be <= `warn_days`.
fn classify(remaining_days: i64, warn_days: u64, crit_days: u64) -> (HealthStatus, String) {
    if remaining_days < 0 {
        return (
            HealthStatus::Unhealthy,
            format!("certificate expired {} day(s) ago", -remaining_days),
        );
    }
    let days = remaining_days as u64;
    let status = if days < crit_days {
        HealthStatus::Unhealthy
    } else if days < warn_days {
        HealthStatus::Degraded
    } else {
        HealthStatus::Healthy
    };
    (status, format!("certificate expires in {days} day(s)"))
}

impl Probe for TlsCertProbe {
    fn name(&self) -> &'static str {
        "tls-cert"
    }

    fn probe(&self, resource: &Resource) -> ProbeReport {
        let start = Instant::now();
        let report = |succeeded, status, observation: String| ProbeReport {
            resource_id: resource.id.clone(),
            probe_name: self.name().to_owned(),
            succeeded,
            status,
            observation,
        };
        let remaining = || self.timeout.saturating_sub(start.elapsed());

        let addresses: Vec<std::net::SocketAddr> =
            match (self.host.as_str(), self.port).to_socket_addrs() {
                Ok(addresses) => addresses.collect(),
                Err(err) => {
                    return report(
                        false,
                        HealthStatus::Unreachable,
                        format!("dns resolution failed: {err}"),
                    );
                }
            };
        if addresses.is_empty() {
            return report(
                false,
                HealthStatus::Unreachable,
                "dns resolution returned no addresses".to_owned(),
            );
        }

        let stream = match connect(&addresses, remaining) {
            Ok(stream) => stream,
            Err((status, observation)) => return report(false, status, observation),
        };

        // A stalled server must not hang the handshake past the deadline:
        // socket timeouts are set before any TLS bytes flow.
        let left = remaining().max(Duration::from_millis(1));
        if let Err(err) = stream
            .set_read_timeout(Some(left))
            .and_then(|_| stream.set_write_timeout(Some(left)))
        {
            return report(
                false,
                HealthStatus::Unreachable,
                format!("set timeout failed: {err}"),
            );
        }

        let handshake = tls::connect_tls(stream, &self.server_name, false);
        let tls_stream = match handshake {
            Ok(stream) => stream,
            Err(err) => {
                // Reached the endpoint but no TLS handshake: a definite
                // misbehavior, not an unreachable probe.
                return report(
                    false,
                    HealthStatus::Unhealthy,
                    format!("tls handshake failed: {err}"),
                );
            }
        };

        let leaf = match tls_stream
            .conn
            .peer_certificates()
            .and_then(|certs| certs.first())
        {
            Some(leaf) => leaf,
            None => {
                return report(
                    false,
                    HealthStatus::Unhealthy,
                    "server presented no certificate".to_owned(),
                );
            }
        };
        let (_, cert) = match parse_x509_certificate(leaf.as_ref()) {
            Ok(parsed) => parsed,
            Err(err) => {
                return report(
                    false,
                    HealthStatus::Unreachable,
                    format!("failed to parse peer certificate: {err}"),
                );
            }
        };

        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let days = days_remaining(cert.validity().not_after.timestamp(), now_secs);
        let (status, observation) = classify(days, self.warn_days, self.crit_days);
        let subject = cert.subject().to_string();
        let not_after = cert.validity().not_after;
        report(
            status == HealthStatus::Healthy,
            status,
            format!("{observation}; subject {subject}, not_after {not_after}"),
        )
    }
}

/// Connects to the first address that accepts, honoring the deadline.
/// Returns (status, observation) on failure: timeout/DNS → Unreachable,
/// connection refused → Unhealthy.
fn connect(
    addresses: &[std::net::SocketAddr],
    mut remaining: impl FnMut() -> Duration,
) -> Result<TcpStream, (HealthStatus, String)> {
    let mut last_error = None;
    for address in addresses {
        let left = remaining();
        if left.is_zero() {
            return Err((
                HealthStatus::Unreachable,
                "connect timed out before any address was tried".to_owned(),
            ));
        }
        match TcpStream::connect_timeout(address, left) {
            Ok(stream) => return Ok(stream),
            Err(err) => {
                if err.kind() == std::io::ErrorKind::TimedOut
                    || err.kind() == std::io::ErrorKind::WouldBlock
                {
                    return Err((HealthStatus::Unreachable, "connect timed out".to_owned()));
                }
                last_error = Some(err);
            }
        }
    }
    Err((
        HealthStatus::Unhealthy,
        format!(
            "connection refused: {}",
            last_error
                .map(|err| err.to_string())
                .unwrap_or_else(|| "no address accepted".to_owned())
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(host: &str, port: u16) -> TlsCertProbe {
        TlsCertProbe {
            host: host.into(),
            port,
            server_name: host.into(),
            timeout: Duration::from_secs(2),
            warn_days: DEFAULT_WARN_DAYS,
            crit_days: DEFAULT_CRIT_DAYS,
        }
    }

    #[test]
    fn days_remaining_is_floor_forward_ceil_backward() {
        let now = 1_000_000;
        assert_eq!(days_remaining(now + 86_400 * 3 + 1, now), 3);
        assert_eq!(days_remaining(now + 86_400 * 3, now), 3);
        assert_eq!(days_remaining(now + 86_399, now), 0);
        assert_eq!(days_remaining(now, now), 0);
        assert_eq!(days_remaining(now - 1, now), -1);
        assert_eq!(days_remaining(now - 86_400, now), -1);
        assert_eq!(days_remaining(now - 86_401, now), -2);
    }

    #[test]
    fn classify_thresholds() {
        // crit_days=7, warn_days=30.
        assert_eq!(classify(-1, 30, 7).0, HealthStatus::Unhealthy);
        assert_eq!(classify(0, 30, 7).0, HealthStatus::Unhealthy);
        assert_eq!(classify(6, 30, 7).0, HealthStatus::Unhealthy);
        assert_eq!(classify(7, 30, 7).0, HealthStatus::Degraded);
        assert_eq!(classify(29, 30, 7).0, HealthStatus::Degraded);
        assert_eq!(classify(30, 30, 7).0, HealthStatus::Healthy);
        assert_eq!(classify(365, 30, 7).0, HealthStatus::Healthy);
    }

    #[test]
    fn classify_is_healthy_when_crit_and_warn_equal() {
        // Equal thresholds: only crit remains (everything under it fails).
        assert_eq!(classify(6, 7, 7).0, HealthStatus::Unhealthy);
        assert_eq!(classify(7, 7, 7).0, HealthStatus::Healthy);
    }

    #[test]
    fn from_config_requires_host() {
        assert!(from_config(&serde_json::json!({})).is_err());
        assert!(from_config(&serde_json::json!({"host": "  "})).is_err());
    }

    #[test]
    fn from_config_defaults_port_and_thresholds() {
        let probe = from_config(&serde_json::json!({"host": "example.com"})).unwrap();
        assert_eq!(probe.port, 443);
        assert_eq!(probe.warn_days, 30);
        assert_eq!(probe.crit_days, 7);
        assert_eq!(probe.server_name, "example.com");
    }

    #[test]
    fn from_config_rejects_crit_above_warn() {
        assert!(from_config(&serde_json::json!({"host": "example.com", "crit_days": 40})).is_err());
    }

    #[test]
    fn from_config_accepts_server_name_override() {
        let probe =
            from_config(&serde_json::json!({"host": "127.0.0.1", "server_name": "example.com"}))
                .unwrap();
        assert_eq!(probe.server_name, "example.com");
    }

    #[test]
    fn closed_port_is_unhealthy() {
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let report = probe("127.0.0.1", port).probe(&Resource::new("tls", "tls-certificate"));
        assert_eq!(report.status, HealthStatus::Unhealthy);
        assert!(!report.succeeded);
    }

    #[test]
    fn bad_hostname_is_unreachable() {
        let report = probe("does not exist", 443).probe(&Resource::new("tls", "tls-certificate"));
        assert_eq!(report.status, HealthStatus::Unreachable);
    }

    #[test]
    fn non_tls_endpoint_is_unhealthy() {
        // Plain TCP listener that accepts but never speaks TLS.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let _ = listener.accept(); // accept and drop immediately
        });
        let report =
            probe("127.0.0.1", addr.port()).probe(&Resource::new("tls", "tls-certificate"));
        assert_eq!(report.status, HealthStatus::Unhealthy);
        assert!(
            report.observation.contains("handshake"),
            "{}",
            report.observation
        );
        handle.join().ok();
    }

    /// Probes the shared TLS test server whose cert expires at `expiry`.
    fn probe_expiring(expiry: time::OffsetDateTime) -> ProbeReport {
        let (addr, handle) = crate::tls::test_util::spawn_tls_server_with_expiry(expiry);
        let mut p = probe("127.0.0.1", addr.port());
        p.server_name = "localhost".into();
        let report = p.probe(&Resource::new("tls", "tls-certificate"));
        handle.join().ok();
        report
    }

    #[test]
    fn cert_far_from_expiry_is_healthy() {
        let report = probe_expiring(time::OffsetDateTime::now_utc() + time::Duration::days(365));
        assert_eq!(
            report.status,
            HealthStatus::Healthy,
            "{}",
            report.observation
        );
        assert!(report.succeeded);
        assert!(
            report.observation.contains("expires in"),
            "{}",
            report.observation
        );
    }

    #[test]
    fn cert_near_expiry_is_degraded() {
        let report = probe_expiring(time::OffsetDateTime::now_utc() + time::Duration::days(15));
        assert_eq!(
            report.status,
            HealthStatus::Degraded,
            "{}",
            report.observation
        );
        assert!(!report.succeeded);
    }

    #[test]
    fn cert_inside_crit_days_is_unhealthy() {
        let report = probe_expiring(time::OffsetDateTime::now_utc() + time::Duration::days(5));
        assert_eq!(
            report.status,
            HealthStatus::Unhealthy,
            "{}",
            report.observation
        );
        assert!(!report.succeeded);
    }

    #[test]
    fn expired_cert_is_unhealthy() {
        let report = probe_expiring(time::OffsetDateTime::now_utc() - time::Duration::days(2));
        assert_eq!(
            report.status,
            HealthStatus::Unhealthy,
            "{}",
            report.observation
        );
        assert!(
            report.observation.contains("expired"),
            "{}",
            report.observation
        );
    }
}
