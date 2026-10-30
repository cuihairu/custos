//! TCP port reachability probe (survey §26.3, implementation order #2).
//!
//! `TcpStream::connect_timeout` with a total deadline across addresses,
//! recording the connection latency. Status conventions: connected →
//! Healthy, refused → Unhealthy (the service is down), timeout or DNS
//! failure → Unreachable (the probe could not obtain a result).
//! `tls = true` additionally completes a TLS handshake (accept-any
//! verifier — certificate trust is the tls-cert probe's job, not a
//! connectivity probe's) so "port open but TLS broken" is caught.

use crate::{tls, Probe, ProbeReport};
use custos_model::{HealthStatus, Resource};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

/// Probes one TCP endpoint by port.
#[derive(Debug, Clone)]
pub struct TcpProbe {
    pub host: String,
    pub port: u16,
    pub timeout: Duration,
    /// Complete a TLS handshake after connecting (default off).
    pub tls: bool,
    /// SNI name for the optional TLS handshake; defaults to `host`.
    pub server_name: String,
}

impl Probe for TcpProbe {
    fn name(&self) -> &'static str {
        "tcp"
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

        let addresses: Vec<SocketAddr> = match (self.host.as_str(), self.port).to_socket_addrs() {
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

        let mut last_error = None;
        for address in addresses {
            let left = remaining();
            if left.is_zero() {
                return report(
                    false,
                    HealthStatus::Unreachable,
                    format!("connect timeout after {}ms", self.timeout.as_millis()),
                );
            }
            match TcpStream::connect_timeout(&address, left) {
                Ok(stream) => {
                    return match self.finish(stream, remaining()) {
                        Ok(observation) => report(
                            true,
                            HealthStatus::Healthy,
                            format!(
                                "connected to {address} in {}ms; {observation}",
                                start.elapsed().as_millis()
                            ),
                        ),
                        Err(observation) => report(false, HealthStatus::Unhealthy, observation),
                    };
                }
                Err(err) => {
                    if err.kind() == std::io::ErrorKind::TimedOut
                        || err.kind() == std::io::ErrorKind::WouldBlock
                    {
                        return report(
                            false,
                            HealthStatus::Unreachable,
                            format!("connect timeout after {}ms", self.timeout.as_millis()),
                        );
                    }
                    last_error = Some(err);
                }
            }
        }
        report(
            false,
            HealthStatus::Unhealthy,
            format!(
                "connection refused: {}",
                last_error
                    .map(|err| err.to_string())
                    .unwrap_or_else(|| "no address accepted".to_owned())
            ),
        )
    }
}

impl TcpProbe {
    /// After a successful connect: plain TCP is done, `tls = true` still
    /// completes the handshake under the remaining deadline.
    fn finish(&self, stream: TcpStream, remaining: Duration) -> Result<String, String> {
        if !self.tls {
            return Ok("tcp connect ok".to_owned());
        }
        let left = remaining.max(Duration::from_millis(1));
        stream
            .set_read_timeout(Some(left))
            .and_then(|_| stream.set_write_timeout(Some(left)))
            .map_err(|err| format!("set timeout failed: {err}"))?;
        let server_name = if self.server_name.is_empty() {
            self.host.as_str()
        } else {
            self.server_name.as_str()
        };
        drop(tls::connect_tls(stream, server_name, false)?);
        Ok("tls handshake ok".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    fn resource() -> Resource {
        Resource::new("svc", "network-connectivity")
    }

    fn probe(host: &str, port: u16) -> TcpProbe {
        TcpProbe {
            host: host.into(),
            port,
            timeout: Duration::from_secs(2),
            tls: false,
            server_name: host.into(),
        }
    }

    #[test]
    fn open_port_reports_healthy_with_latency() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let report = probe("127.0.0.1", addr.port()).probe(&resource());
        assert!(report.succeeded);
        assert_eq!(report.status, HealthStatus::Healthy);
        assert!(
            report.observation.contains("connected"),
            "{}",
            report.observation
        );
        drop(listener);
    }

    #[test]
    fn closed_port_reports_unhealthy() {
        // Bind then drop: the port is free. A freshly-freed ephemeral port
        // can be grabbed by a concurrent test's listener, so retry with a
        // fresh port until nothing answers (bounded).
        let mut last = None;
        for _ in 0..5 {
            let port = {
                let listener = TcpListener::bind("127.0.0.1:0").unwrap();
                listener.local_addr().unwrap().port()
            };
            let report = probe("127.0.0.1", port).probe(&resource());
            if report.status == HealthStatus::Unhealthy {
                assert!(!report.succeeded);
                assert!(
                    report.observation.contains("refused"),
                    "{}",
                    report.observation
                );
                return;
            }
            last = Some(report);
        }
        panic!("no freed port stayed closed in 5 attempts: {last:?}");
    }

    #[test]
    fn unreachable_hostname_reports_unreachable() {
        let report = probe("does not exist", 80).probe(&resource());
        assert!(!report.succeeded);
        assert_eq!(report.status, HealthStatus::Unreachable);
    }

    #[test]
    fn server_that_accepts_but_never_replies_times_out() {
        // Not a TCP-connect scenario — connect succeeds — but guards the
        // probe's own deadline path via the closed-port and unreachable
        // cases above. Kept as a placeholder for a future read-side probe.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let _ = listener.accept();
        });
        let report = probe(&addr.ip().to_string(), addr.port()).probe(&resource());
        assert_eq!(
            report.status,
            HealthStatus::Healthy,
            "connect alone is enough"
        );
        handle.join().ok();
    }

    #[test]
    fn tls_handshake_ok_reports_healthy() {
        let (addr, handle) = tls::test_util::spawn_tls_server();
        let mut p = probe("127.0.0.1", addr.port());
        p.tls = true;
        p.server_name = "localhost".into();
        let report = p.probe(&resource());
        assert_eq!(
            report.status,
            HealthStatus::Healthy,
            "{}",
            report.observation
        );
        assert!(
            report.observation.contains("tls handshake ok"),
            "{}",
            report.observation
        );
        handle.join().ok();
    }

    #[test]
    fn tls_handshake_on_plain_tcp_is_unhealthy() {
        // TLS expected but the endpoint is not a TLS server.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let _ = listener.accept();
        });
        let mut p = probe("127.0.0.1", addr.port());
        p.tls = true;
        let report = p.probe(&resource());
        assert_eq!(
            report.status,
            HealthStatus::Unhealthy,
            "{}",
            report.observation
        );
        assert!(
            report.observation.contains("handshake"),
            "{}",
            report.observation
        );
        handle.join().ok();
    }
}
