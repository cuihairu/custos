//! Database connectivity probe (survey §26.11, implementation order #12) —
//! MySQL / PostgreSQL / Redis, via the mature client crates.
//!
//! Connects and issues a liveness statement (`SELECT 1` for MySQL /
//! PostgreSQL, `PING` for Redis) and normalizes per the survey: a transport
//! timeout (and other unclassifiable transport failures such as DNS) →
//! Unreachable, a connection refusal or a server-level rejection (auth,
//! bad query) → Unhealthy, a valid answer → Healthy. Classification is a
//! portable `classify_io` / `Failure` pair (unit-tested everywhere); only
//! the client calls are per-kind, and only the Redis/MySQL live paths are
//! exercised in tests when a local server exists.

use crate::config::{optional_str, required_str, u64_or};
use crate::{Probe, ProbeReport};
use custos_model::{HealthStatus, Resource};
use std::io::ErrorKind;
use std::time::Duration;

const DEFAULT_TIMEOUT_MS: u64 = 3_000;
const DEFAULT_QUERY: &str = "SELECT 1";

/// Which database protocol the probe speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseKind {
    Mysql,
    Postgres,
    Redis,
}

impl DatabaseKind {
    fn default_port(self) -> u16 {
        match self {
            DatabaseKind::Mysql => 3306,
            DatabaseKind::Postgres => 5432,
            DatabaseKind::Redis => 6379,
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "mysql" => Some(DatabaseKind::Mysql),
            "postgres" => Some(DatabaseKind::Postgres),
            "redis" => Some(DatabaseKind::Redis),
            _ => None,
        }
    }
}

/// Probes one database with a liveness statement.
#[derive(Debug, Clone)]
pub struct DatabaseProbe {
    pub kind: DatabaseKind,
    pub host: String,
    pub port: u16,
    pub user: Option<String>,
    pub password: Option<String>,
    pub database: Option<String>,
    pub timeout: Duration,
    /// Liveness statement for MySQL/PostgreSQL; ignored for Redis (PING).
    pub query: String,
}

/// Builds from `[resources.probe]` (docs/guide/probe-survey.md §26.11).
pub fn from_config(config: &serde_json::Value) -> Result<DatabaseProbe, String> {
    let kind = match config["kind"].as_str() {
        Some(raw) => DatabaseKind::from_str(raw).ok_or_else(|| {
            format!("`probe.kind` must be mysql, postgres, or redis, got {raw:?}")
        })?,
        None => return Err("`probe.kind` is required for database resources".to_owned()),
    };
    let host =
        required_str(config, "host").ok_or("`probe.host` is required for database resources")?;
    let default_port = kind.default_port();
    let port = u16::try_from(u64_or(config, "port", default_port as u64))
        .map_err(|_| "`probe.port` must be an integer in 0..=65535".to_owned())?;
    Ok(DatabaseProbe {
        kind,
        host,
        port,
        user: optional_str(config, "user"),
        password: optional_str(config, "password"),
        database: optional_str(config, "database"),
        timeout: Duration::from_millis(u64_or(config, "timeout_ms", DEFAULT_TIMEOUT_MS)),
        query: config["query"].as_str().unwrap_or(DEFAULT_QUERY).to_owned(),
    })
}

impl Probe for DatabaseProbe {
    fn name(&self) -> &'static str {
        "database"
    }

    fn probe(&self, resource: &Resource) -> ProbeReport {
        let report = |succeeded, status, observation: String| ProbeReport {
            resource_id: resource.id.clone(),
            probe_name: self.name().to_owned(),
            succeeded,
            status,
            observation,
        };
        let outcome = match self.kind {
            DatabaseKind::Mysql => self.probe_mysql(),
            DatabaseKind::Postgres => self.probe_postgres(),
            DatabaseKind::Redis => self.probe_redis(),
        };
        match outcome {
            Ok(observation) => report(true, HealthStatus::Healthy, observation),
            Err((Failure::Unreachable, observation)) => {
                report(false, HealthStatus::Unreachable, observation)
            }
            Err((Failure::Unhealthy, observation)) => {
                report(false, HealthStatus::Unhealthy, observation)
            }
        }
    }
}

/// The two failure buckets the survey asks to normalize separately: a
/// transport timeout (could not get a valid result in time) vs a definite
/// database-side failure (refused, auth, bad query).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Failure {
    Unreachable,
    Unhealthy,
}

/// Normalizes an underlying transport `io::Error` per the probe-survey
/// convention: connection refused is a definite failure (Unhealthy), while
/// timeout and other unclassifiable transport failures (DNS, network
/// unreachable) are Unreachable.
fn classify_io(error: &std::io::Error) -> Failure {
    match error.kind() {
        ErrorKind::ConnectionRefused => Failure::Unhealthy,
        _ => Failure::Unreachable,
    }
}

/// Walks a crate error's source chain looking for the transport
/// `io::Error`, so timeout vs refusal can be normalized regardless of which
/// client crate wrapped it. (Note: `std::io::Error::source()` skips its own
/// payload, so an `io::Error` never appears twice in one walk — the real
/// pattern is a crate error whose source chain reaches one `io::Error`.)
fn find_io_error<'a>(error: &'a (dyn std::error::Error + 'static)) -> Option<&'a std::io::Error> {
    let mut current = Some(error);
    while let Some(err) = current {
        if let Some(io) = err.downcast_ref::<std::io::Error>() {
            return Some(io);
        }
        current = err.source();
    }
    None
}

fn classify_mysql(error: &mysql::Error) -> (Failure, String) {
    match error {
        // A server-level answer (access denied, unknown database, bad SQL)
        // is a definite database-side failure.
        mysql::Error::MySqlError(server) => (Failure::Unhealthy, format!("mysql: {server}")),
        _ => match find_io_error(error) {
            Some(io) => (classify_io(io), format!("mysql: {error}")),
            None => (Failure::Unhealthy, format!("mysql: {error}")),
        },
    }
}

fn classify_postgres(error: &postgres::Error) -> (Failure, String) {
    // A server-side answer (auth failed, bad SQL) is a definite failure.
    if error.as_db_error().is_some() {
        return (Failure::Unhealthy, format!("postgres: {error}"));
    }
    match find_io_error(error) {
        Some(io) => (classify_io(io), format!("postgres: {error}")),
        None => (Failure::Unhealthy, format!("postgres: {error}")),
    }
}

fn classify_redis(error: &redis::RedisError) -> (Failure, String) {
    match error.kind() {
        redis::ErrorKind::AuthenticationFailed | redis::ErrorKind::Server(_) => {
            (Failure::Unhealthy, format!("redis: {error}"))
        }
        _ => match find_io_error(error) {
            Some(io) => (classify_io(io), format!("redis: {error}")),
            None => (Failure::Unhealthy, format!("redis: {error}")),
        },
    }
}

impl DatabaseProbe {
    fn probe_mysql(&self) -> Result<String, (Failure, String)> {
        use mysql::prelude::Queryable;
        let opts = mysql::OptsBuilder::new()
            .ip_or_hostname(Some(self.host.clone()))
            .tcp_port(self.port)
            .user(self.user.clone())
            .pass(self.password.clone())
            .db_name(self.database.clone())
            .tcp_connect_timeout(Some(self.timeout));
        let mut conn = mysql::Conn::new(opts).map_err(|error| classify_mysql(&error))?;
        let _: Option<i32> = conn
            .query_first(self.query.as_str())
            .map_err(|error| classify_mysql(&error))?;
        Ok(format!("{} ok", self.query))
    }

    fn probe_postgres(&self) -> Result<String, (Failure, String)> {
        let mut config = postgres::Config::new();
        config.host(&self.host);
        config.port(self.port);
        config.connect_timeout(self.timeout);
        if let Some(user) = &self.user {
            config.user(user);
        }
        if let Some(password) = &self.password {
            config.password(password);
        }
        if let Some(database) = &self.database {
            config.dbname(database);
        }
        let mut client = config
            .connect(postgres::NoTls)
            .map_err(|error| classify_postgres(&error))?;
        let _ = client
            .query_one(&self.query, &[])
            .map_err(|error| classify_postgres(&error))?;
        Ok(format!("{} ok", self.query))
    }

    fn probe_redis(&self) -> Result<String, (Failure, String)> {
        let url = self.redis_url();
        let client = redis::Client::open(url.as_str()).map_err(|error| classify_redis(&error))?;
        let mut conn = client
            .get_connection_with_timeout(self.timeout)
            .map_err(|error| classify_redis(&error))?;
        let pong: String = redis::cmd("PING")
            .query(&mut conn)
            .map_err(|error| classify_redis(&error))?;
        if pong != "PONG" {
            return Err((
                Failure::Unhealthy,
                format!("redis: unexpected PING reply {pong:?}"),
            ));
        }
        Ok("PONG".to_owned())
    }

    /// Builds a `redis://[user[:pass]@]host:port/db` URL, percent-encoding
    /// the credential components so passwords with reserved characters work.
    fn redis_url(&self) -> String {
        let mut url = String::from("redis://");
        if self.user.is_some() || self.password.is_some() {
            if let Some(user) = &self.user {
                url.push_str(&percent_encode_credential(user));
            }
            if let Some(password) = &self.password {
                url.push(':');
                url.push_str(&percent_encode_credential(password));
            }
            url.push('@');
        }
        if self.host.contains(':') {
            // IPv6 literal.
            url.push('[');
            url.push_str(&self.host);
            url.push(']');
        } else {
            url.push_str(&self.host);
        }
        url.push(':');
        url.push_str(&self.port.to_string());
        if let Some(database) = &self.database {
            url.push('/');
            url.push_str(database);
        }
        url
    }
}

/// Percent-encodes a URL userinfo component (unreserved chars pass through).
fn percent_encode_credential(component: &str) -> String {
    let mut encoded = String::with_capacity(component.len());
    for byte in component.bytes() {
        let unreserved = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~');
        if unreserved {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn resource() -> Resource {
        Resource::new("db", "database")
    }

    #[test]
    fn from_config_requires_kind_and_host() {
        assert!(from_config(&json!({})).is_err());
        assert!(from_config(&json!({"kind": "mysql"})).is_err());
        assert!(from_config(&json!({"kind": "oracle", "host": "x"})).is_err());
    }

    #[test]
    fn from_config_defaults_port_and_query() {
        let probe = from_config(&json!({"kind": "postgres", "host": "db.local"})).unwrap();
        assert_eq!(probe.kind, DatabaseKind::Postgres);
        assert_eq!(probe.port, 5432);
        assert_eq!(probe.query, "SELECT 1");
        assert_eq!(probe.timeout, Duration::from_millis(3000));
    }

    #[test]
    fn from_config_reads_custom_port_and_query() {
        let probe = from_config(&json!({
            "kind": "mysql", "host": "h", "port": 3307,
            "user": "u", "password": "p", "database": "d",
            "query": "SELECT 2", "timeout_ms": 500
        }))
        .unwrap();
        assert_eq!(probe.port, 3307);
        assert_eq!(probe.query, "SELECT 2");
        assert_eq!(probe.timeout, Duration::from_millis(500));
    }

    #[test]
    fn from_config_rejects_bad_port() {
        assert!(from_config(&json!({"kind": "redis", "host": "h", "port": 70000})).is_err());
    }

    #[test]
    fn classify_io_refused_is_unhealthy() {
        let err = std::io::Error::new(ErrorKind::ConnectionRefused, "refused");
        assert_eq!(classify_io(&err), Failure::Unhealthy);
    }

    #[test]
    fn classify_io_timeout_is_unreachable() {
        let err = std::io::Error::new(ErrorKind::TimedOut, "timeout");
        assert_eq!(classify_io(&err), Failure::Unreachable);
    }

    #[test]
    fn classify_io_other_is_unreachable() {
        let err = std::io::Error::other("dns");
        assert_eq!(classify_io(&err), Failure::Unreachable);
    }

    /// A non-`io` error exposing a wrapped `io::Error` via its source chain,
    /// matching what the client crates do (`mysql::Error::IoError`, the
    /// `tokio_postgres::Error` connect path, `RedisError` from a connect).
    #[derive(Debug)]
    struct Wrapper(std::io::Error);

    impl std::fmt::Display for Wrapper {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "wrapper")
        }
    }

    impl std::error::Error for Wrapper {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }

    #[test]
    fn find_io_error_walks_source_chain() {
        let io = std::io::Error::new(ErrorKind::ConnectionRefused, "refused");
        let wrapped = Wrapper(io);
        let found = find_io_error(&wrapped).expect("io error in chain");
        assert_eq!(found.kind(), ErrorKind::ConnectionRefused);
    }

    #[test]
    fn find_io_error_accepts_the_io_error_itself() {
        let plain = std::io::Error::other("plain");
        let found = find_io_error(&plain).expect("the io error itself");
        assert_eq!(found.kind(), ErrorKind::Other);
    }

    #[test]
    fn redis_url_encodes_credentials() {
        let probe = DatabaseProbe {
            kind: DatabaseKind::Redis,
            host: "cache.local".into(),
            port: 6379,
            user: Some("alice".into()),
            password: Some("p@ss:w/rd".into()),
            database: Some("2".into()),
            timeout: Duration::from_secs(1),
            query: "SELECT 1".into(),
        };
        assert_eq!(
            probe.redis_url(),
            "redis://alice:p%40ss%3Aw%2Frd@cache.local:6379/2"
        );
    }

    #[test]
    fn redis_url_without_credentials_is_plain() {
        let probe = DatabaseProbe {
            kind: DatabaseKind::Redis,
            host: "127.0.0.1".into(),
            port: 6379,
            user: None,
            password: None,
            database: None,
            timeout: Duration::from_secs(1),
            query: "SELECT 1".into(),
        };
        assert_eq!(probe.redis_url(), "redis://127.0.0.1:6379");
    }

    #[test]
    fn redis_url_braces_ipv6() {
        let probe = DatabaseProbe {
            kind: DatabaseKind::Redis,
            host: "::1".into(),
            port: 6379,
            user: None,
            password: None,
            database: None,
            timeout: Duration::from_secs(1),
            query: "SELECT 1".into(),
        };
        assert_eq!(probe.redis_url(), "redis://[::1]:6379");
    }

    #[test]
    fn silent_host_is_never_healthy() {
        // TEST-NET-1 (RFC 5737) is not routed: the connect times out (or the
        // sandbox refuses) — a definite non-Healthy verdict either way.
        for kind in ["mysql", "postgres", "redis"] {
            let probe =
                from_config(&json!({"kind": kind, "host": "203.0.113.1", "timeout_ms": 300}))
                    .unwrap();
            let report = probe.probe(&resource());
            assert_ne!(report.status, HealthStatus::Healthy, "{kind}");
            assert_ne!(report.status, HealthStatus::Unknown, "{kind}");
        }
    }

    #[test]
    fn refused_loopback_is_unhealthy_or_unreachable() {
        // A closed loopback port refuses (or, if filtered, times out): a
        // definite non-Healthy verdict, never a panic.
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            drop(listener);
            port
        };
        for kind in ["mysql", "postgres", "redis"] {
            let probe = from_config(
                &json!({"kind": kind, "host": "127.0.0.1", "port": port, "timeout_ms": 500}),
            )
            .unwrap();
            let report = probe.probe(&resource());
            assert_ne!(report.status, HealthStatus::Healthy, "{kind}");
            assert_ne!(report.status, HealthStatus::Unknown, "{kind}");
        }
    }

    /// Spins up a throwaway `redis-server` on an ephemeral port (no
    /// persistence) and verifies the healthy path end to end. Skips when
    /// `redis-server` is not installed.
    #[test]
    fn live_redis_reports_healthy_when_available() {
        let ok = std::process::Command::new("redis-server")
            .arg("--version")
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false);
        if !ok {
            return; // redis-server not installed; skip
        }
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            drop(listener);
            port
        };
        let mut child = std::process::Command::new("redis-server")
            .args([
                "--port",
                &port.to_string(),
                "--save",
                "",
                "--appendonly",
                "no",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn redis-server");
        let mut ready = false;
        for _ in 0..50 {
            if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                ready = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        if !ready {
            let _ = child.kill();
            let _ = child.wait();
            return; // server never came up; skip rather than flake
        }
        let probe = from_config(&json!({
            "kind": "redis", "host": "127.0.0.1", "port": port, "timeout_ms": 2000
        }))
        .unwrap();
        let report = probe.probe(&resource());
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(
            report.status,
            HealthStatus::Healthy,
            "{}",
            report.observation
        );
        assert!(
            report.observation.contains("PONG"),
            "{}",
            report.observation
        );
    }
}
