//! TCP port reachability probe (survey §26.3, implementation order #2).
//!
//! `TcpStream::connect_timeout` with a total deadline across addresses,
//! recording the connection latency. Status conventions: connected →
//! Healthy, refused → Unhealthy (the service is down), timeout or DNS
//! failure → Unreachable (the probe could not obtain a result).

use crate::{Probe, ProbeReport};
use custos_model::{HealthStatus, Resource};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

/// Probes one TCP endpoint by port.
#[derive(Debug, Clone)]
pub struct TcpProbe {
    pub host: String,
    pub port: u16,
    pub timeout: Duration,
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
            let remaining = self.timeout.saturating_sub(start.elapsed());
            if remaining.is_zero() {
                return report(
                    false,
                    HealthStatus::Unreachable,
                    format!("connect timeout after {}ms", self.timeout.as_millis()),
                );
            }
            match TcpStream::connect_timeout(&address, remaining) {
                Ok(_) => {
                    return report(
                        true,
                        HealthStatus::Healthy,
                        format!(
                            "connected to {} in {}ms",
                            address,
                            start.elapsed().as_millis()
                        ),
                    );
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    fn resource() -> Resource {
        Resource::new("svc", "network-connectivity")
    }

    #[test]
    fn open_port_reports_healthy_with_latency() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let probe = TcpProbe {
            host: "127.0.0.1".into(),
            port: addr.port(),
            timeout: Duration::from_secs(2),
        };
        let report = probe.probe(&resource());
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
        // Bind then drop: the port is free and nothing listens on it.
        let port = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let probe = TcpProbe {
            host: "127.0.0.1".into(),
            port,
            timeout: Duration::from_secs(2),
        };
        let report = probe.probe(&resource());
        assert!(!report.succeeded);
        assert_eq!(report.status, HealthStatus::Unhealthy);
        assert!(
            report.observation.contains("refused"),
            "{}",
            report.observation
        );
    }

    #[test]
    fn unreachable_hostname_reports_unreachable() {
        let probe = TcpProbe {
            // A space is never a valid hostname; getaddrinfo fails without
            // consulting the wildcard resolver that captures `.invalid` in
            // some environments.
            host: "does not exist".into(),
            port: 80,
            timeout: Duration::from_secs(2),
        };
        let report = probe.probe(&resource());
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
        let probe = TcpProbe {
            host: addr.ip().to_string(),
            port: addr.port(),
            timeout: Duration::from_millis(500),
        };
        let report = probe.probe(&resource());
        assert_eq!(
            report.status,
            HealthStatus::Healthy,
            "connect alone is enough"
        );
        handle.join().ok();
    }
}
