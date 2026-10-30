//! DNS resolution probe (survey §26.4, implementation order #3).
//!
//! A minimal hand-rolled UDP client (no resolver library — the workspace
//! stays dependency-light): builds one query, sends it to an explicit
//! resolver, parses answers with name-compression support, and asserts an
//! optional regex against the answer records. NXDOMAIN/SERVFAIL are a
//! definitive answer (Unhealthy); no response is Unreachable.

use crate::config::{optional_str, required_str, u64_or};
use crate::{Probe, ProbeReport};
use custos_model::{HealthStatus, Resource};
use regex::Regex;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::time::Duration;

const TYPE_A: u16 = 1;
const TYPE_CNAME: u16 = 5;
const TYPE_AAAA: u16 = 28;
const CLASS_IN: u16 = 1;
const MAX_JUMPS: usize = 16;
const MAX_RESPONSE_BYTES: usize = 4096;

/// Record types the probe understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordType {
    A,
    Aaaa,
    Cname,
}

impl RecordType {
    fn code(self) -> u16 {
        match self {
            RecordType::A => TYPE_A,
            RecordType::Aaaa => TYPE_AAAA,
            RecordType::Cname => TYPE_CNAME,
        }
    }

    fn label(self) -> &'static str {
        match self {
            RecordType::A => "A",
            RecordType::Aaaa => "AAAA",
            RecordType::Cname => "CNAME",
        }
    }
}

/// Probes DNS resolution of one name against one resolver.
#[derive(Debug, Clone)]
pub struct DnsProbe {
    pub name: String,
    pub server: SocketAddr,
    pub record_type: RecordType,
    pub expect: Option<Regex>,
    pub timeout: Duration,
}

/// Builds from `[resources.probe]` (docs/guide/probe-survey.md §26.4).
/// `server` defaults to the first nameserver in /etc/resolv.conf, then to
/// a well-known public resolver.
pub fn from_config(config: &serde_json::Value) -> Result<DnsProbe, String> {
    let name = required_str(config, "name").ok_or("`probe.name` is required for dns probes")?;
    if name.trim().is_empty() {
        return Err("`probe.name` must not be empty".to_owned());
    }
    let record_type = match optional_str(config, "record_type")
        .unwrap_or_else(|| "A".to_owned())
        .to_ascii_uppercase()
        .as_str()
    {
        "A" => RecordType::A,
        "AAAA" => RecordType::Aaaa,
        "CNAME" => RecordType::Cname,
        other => {
            return Err(format!(
                "unsupported `probe.record_type` {other:?} (A, AAAA or CNAME)"
            ))
        }
    };
    let expect = match optional_str(config, "expect") {
        Some(pattern) => Some(
            Regex::new(&pattern).map_err(|err| format!("invalid `probe.expect` regex: {err}"))?,
        ),
        None => None,
    };
    let server = match optional_str(config, "server") {
        Some(server) => {
            let with_port = if server.contains(':') {
                server.clone()
            } else {
                format!("{server}:53")
            };
            with_port
                .parse::<SocketAddr>()
                .map_err(|err| format!("invalid `probe.server` {server:?}: {err}"))?
        }
        None => system_resolver()
            .ok_or("no `probe.server` and no nameserver found in /etc/resolv.conf")?,
    };
    Ok(DnsProbe {
        name: name.trim_end_matches('.').to_owned(),
        server,
        record_type,
        expect,
        timeout: Duration::from_millis(u64_or(config, "timeout_ms", 3_000)),
    })
}

/// First nameserver from /etc/resolv.conf (Unix); Windows uses the system
/// resolver path, so fall back to a public resolver there.
fn system_resolver() -> Option<SocketAddr> {
    #[cfg(unix)]
    {
        let Ok(content) = std::fs::read_to_string("/etc/resolv.conf") else {
            return "1.1.1.1:53".parse().ok();
        };
        for line in content.lines() {
            if let Some(rest) = line.trim().strip_prefix("nameserver") {
                let ip = rest.trim();
                if let Ok(addr) = format!("{ip}:53").parse::<SocketAddr>() {
                    return Some(addr);
                }
            }
        }
        "1.1.1.1:53".parse().ok()
    }
    #[cfg(not(unix))]
    {
        "1.1.1.1:53".parse().ok()
    }
}

impl Probe for DnsProbe {
    fn name(&self) -> &'static str {
        "dns"
    }

    fn probe(&self, resource: &Resource) -> ProbeReport {
        let report = |succeeded, status, observation: String| ProbeReport {
            resource_id: resource.id.clone(),
            probe_name: self.name().to_owned(),
            succeeded,
            status,
            observation,
        };
        let answers = match self.exchange() {
            Ok(answers) => answers,
            Err((status, message)) => return report(false, status, message),
        };
        if answers.is_empty() {
            return report(
                false,
                HealthStatus::Unhealthy,
                format!("no {} records for {}", self.record_type.label(), self.name),
            );
        }
        let rendered = answers.join(", ");
        if let Some(expect) = &self.expect {
            if !expect.is_match(&rendered) {
                return report(
                    false,
                    HealthStatus::Unhealthy,
                    format!(
                        "{} records for {} do not match {expect}: [{rendered}]",
                        self.record_type.label(),
                        self.name
                    ),
                );
            }
        }
        report(
            true,
            HealthStatus::Healthy,
            format!(
                "{} {} -> [{}] via {}",
                self.record_type.label(),
                self.name,
                clip(&rendered),
                self.server
            ),
        )
    }
}

fn clip(text: &str) -> String {
    text.chars().take(200).collect()
}

impl DnsProbe {
    /// Sends one query and collects the answers of the requested type.
    /// The status pair distinguishes probe failures (Unreachable) from
    /// definitive negative answers (Unhealthy, e.g. NXDOMAIN).
    fn exchange(&self) -> Result<Vec<String>, (HealthStatus, String)> {
        let query = build_query(&self.name, self.record_type.code())
            .map_err(|err| (HealthStatus::Unreachable, err))?;
        let socket = UdpSocket::bind("0.0.0.0:0").map_err(|err| {
            (
                HealthStatus::Unreachable,
                format!("cannot bind udp socket: {err}"),
            )
        })?;
        socket.set_read_timeout(Some(self.timeout)).map_err(|err| {
            (
                HealthStatus::Unreachable,
                format!("set read timeout: {err}"),
            )
        })?;
        socket.connect(self.server).map_err(|err| {
            (
                HealthStatus::Unreachable,
                format!("cannot reach resolver {}: {err}", self.server),
            )
        })?;
        socket.send(&query).map_err(|err| {
            (
                HealthStatus::Unreachable,
                format!("send to {} failed: {err}", self.server),
            )
        })?;
        let mut buf = [0u8; MAX_RESPONSE_BYTES];
        let received = loop {
            match socket.recv(&mut buf) {
                Ok(len) => break len,
                // EINTR: a signal interrupted recv; retry rather than
                // misreporting a healthy resolver as unreachable.
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(err) => {
                    if matches!(
                        err.kind(),
                        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                    ) {
                        return Err((
                            HealthStatus::Unreachable,
                            format!(
                                "no response from resolver {} after {}ms",
                                self.server,
                                self.timeout.as_millis()
                            ),
                        ));
                    }
                    return Err((
                        HealthStatus::Unreachable,
                        format!("recv from {} failed: {err}", self.server),
                    ));
                }
            }
        };
        let packet = &buf[..received];
        let id = u16::from_be_bytes([query[0], query[1]]);
        let response = parse_response(packet, id, self.record_type.code()).map_err(|err| {
            (
                HealthStatus::Unreachable,
                format!("malformed dns response: {err}"),
            )
        })?;
        match response.rcode {
            Rcode::NoError => Ok(response.answers),
            Rcode::NxDomain => Err((
                HealthStatus::Unhealthy,
                format!(
                    "{} does not exist (NXDOMAIN from {})",
                    self.name, self.server
                ),
            )),
            other => Err((
                HealthStatus::Unhealthy,
                format!("resolver returned {} for {}", other.as_str(), self.name),
            )),
        }
    }
}

/// RCODEs the probe distinguishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rcode {
    NoError,
    NxDomain,
    Other(u8),
}

impl Rcode {
    fn as_str(self) -> &'static str {
        match self {
            Rcode::NoError => "NOERROR",
            Rcode::NxDomain => "NXDOMAIN",
            Rcode::Other(1) => "FORMERR",
            Rcode::Other(2) => "SERVFAIL",
            Rcode::Other(5) => "REFUSED",
            Rcode::Other(_) => "error",
        }
    }
}

/// Builds a recursive query message with a random transaction id.
fn build_query(name: &str, qtype: u16) -> Result<Vec<u8>, String> {
    let id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u16)
        .unwrap_or(0x3C3C);
    let mut message = Vec::with_capacity(64);
    message.extend_from_slice(&id.to_be_bytes());
    message.extend_from_slice(&0x0100u16.to_be_bytes()); // RD
    message.extend_from_slice(&1u16.to_be_bytes()); // qdcount
    message.extend_from_slice(&0u16.to_be_bytes()); // ancount
    message.extend_from_slice(&0u16.to_be_bytes()); // nscount
    message.extend_from_slice(&0u16.to_be_bytes()); // arcount
    encode_name(&mut message, name)?;
    message.extend_from_slice(&qtype.to_be_bytes());
    message.extend_from_slice(&CLASS_IN.to_be_bytes());
    Ok(message)
}

/// Encodes a dotted name into length-prefixed labels.
fn encode_name(out: &mut Vec<u8>, name: &str) -> Result<(), String> {
    for label in name.trim_end_matches('.').split('.') {
        if label.is_empty() {
            return Err(format!("empty label in name {name:?}"));
        }
        if label.len() > 63 {
            return Err(format!("label too long in name {name:?}"));
        }
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
    Ok(())
}

#[derive(Debug)]
struct Response {
    rcode: Rcode,
    answers: Vec<String>,
}

/// Parses a response header + question + answer section, following name
/// compression pointers with a jump budget so loops cannot hang the probe.
fn parse_response(packet: &[u8], expected_id: u16, qtype: u16) -> Result<Response, String> {
    if packet.len() < 12 {
        return Err("short header".to_owned());
    }
    let id = u16::from_be_bytes([packet[0], packet[1]]);
    if id != expected_id {
        return Err(format!("id mismatch (got {id}, want {expected_id})"));
    }
    let flags = u16::from_be_bytes([packet[2], packet[3]]);
    if flags & 0x8000 == 0 {
        return Err("message is not a response".to_owned());
    }
    let rcode = match flags & 0x000F {
        0 => Rcode::NoError,
        3 => Rcode::NxDomain,
        other => Rcode::Other(other as u8),
    };
    let qdcount = u16::from_be_bytes([packet[4], packet[5]]);
    let ancount = u16::from_be_bytes([packet[6], packet[7]]);

    let mut offset = 12;
    for _ in 0..qdcount {
        let (_, next) = read_name(packet, offset)?;
        offset = next + 4; // qtype + qclass
        if offset > packet.len() {
            return Err("truncated question".to_owned());
        }
    }

    let mut answers = Vec::new();
    for _ in 0..ancount {
        let (name, next) = read_name(packet, offset)?;
        offset = next;
        if offset + 10 > packet.len() {
            return Err("truncated record header".to_owned());
        }
        let rtype = u16::from_be_bytes([packet[offset], packet[offset + 1]]);
        // class at offset+2..4, ttl at offset+4..8
        let rdlength = u16::from_be_bytes([packet[offset + 8], packet[offset + 9]]) as usize;
        offset += 10;
        let rdata = packet
            .get(offset..offset + rdlength)
            .ok_or("truncated rdata")?;
        offset += rdlength;
        if rtype != qtype {
            continue;
        }
        let value = match rtype {
            TYPE_A => {
                if rdata.len() != 4 {
                    return Err("short A rdata".to_owned());
                }
                Ipv4Addr::new(rdata[0], rdata[1], rdata[2], rdata[3]).to_string()
            }
            TYPE_AAAA => {
                if rdata.len() != 16 {
                    return Err("short AAAA rdata".to_owned());
                }
                let mut octets = [0u8; 16];
                octets.copy_from_slice(rdata);
                Ipv6Addr::from(octets).to_string()
            }
            TYPE_CNAME => {
                let (target, _) = read_name(packet, offset - rdlength)?;
                target
            }
            _ => continue,
        };
        // `name` is the owner of this answer; keep it for CNAME chains.
        let _ = name;
        answers.push(value);
    }
    Ok(Response { rcode, answers })
}

/// Reads a (possibly compressed) name; returns the text and the offset
/// just past the name **at the current position**.
fn read_name(packet: &[u8], start: usize) -> Result<(String, usize), String> {
    let mut labels = Vec::new();
    let mut offset = start;
    let mut jumps = 0;
    let mut resume = None;
    loop {
        let len = *packet.get(offset).ok_or("truncated name")?;
        match len {
            0 => {
                offset += 1;
                break;
            }
            byte if byte & 0xC0 == 0xC0 => {
                let low = *packet.get(offset + 1).ok_or("truncated pointer")? as usize;
                let pointer = (((byte as usize) & 0x3F) << 8) | low;
                if jumps == 0 {
                    resume = Some(offset + 2);
                }
                jumps += 1;
                if jumps > MAX_JUMPS {
                    return Err("compression loop".to_owned());
                }
                offset = pointer;
            }
            byte if byte & 0xC0 == 0 => {
                let end = offset + 1 + byte as usize;
                let label = packet.get(offset + 1..end).ok_or("truncated label")?;
                labels.push(String::from_utf8_lossy(label).into_owned());
                offset = end;
            }
            byte => return Err(format!("bad label length byte {byte}")),
        }
    }
    Ok((labels.join("."), resume.unwrap_or(offset)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use std::thread;

    fn resource() -> Resource {
        Resource::new("dns-api", "dns")
    }

    fn probe_with(server: SocketAddr, name: &str, record_type: RecordType) -> DnsProbe {
        DnsProbe {
            name: name.to_owned(),
            server,
            record_type,
            expect: None,
            timeout: Duration::from_millis(500),
        }
    }

    /// Serves one canned response for any query.
    fn fake_resolver<F>(respond: F) -> SocketAddr
    where
        F: FnOnce(Vec<u8>) -> Vec<u8> + Send + 'static,
    {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = socket.local_addr().unwrap();
        thread::spawn(move || {
            let mut buf = [0u8; MAX_RESPONSE_BYTES];
            if let Ok((len, src)) = socket.recv_from(&mut buf) {
                let response = respond(buf[..len].to_vec());
                let _ = socket.send_to(&response, src);
            }
        });
        addr
    }

    /// Echoes the query question and answers with one A record.
    fn answer_a(ip: [u8; 4]) -> impl FnOnce(Vec<u8>) -> Vec<u8> {
        move |query| {
            let mut response = query.clone();
            response[2] = 0x81; // QR + RD + RA
            response[3] = 0x80; // rcode 0
            response[6] = 0;
            response[7] = 1; // ancount = 1
                             // Answer: pointer to the question name at offset 12.
            response.extend_from_slice(&[0xC0, 0x0C]);
            response.extend_from_slice(&TYPE_A.to_be_bytes());
            response.extend_from_slice(&CLASS_IN.to_be_bytes());
            response.extend_from_slice(&60u32.to_be_bytes());
            response.extend_from_slice(&4u16.to_be_bytes());
            response.extend_from_slice(&ip);
            response
        }
    }

    #[test]
    fn a_record_answer_is_healthy() {
        let server = fake_resolver(answer_a([93, 184, 216, 34]));
        let probe = probe_with(server, "example.test", RecordType::A);
        let report = probe.probe(&resource());
        assert!(report.succeeded, "{}", report.observation);
        assert_eq!(report.status, HealthStatus::Healthy);
        assert!(
            report.observation.contains("93.184.216.34"),
            "{}",
            report.observation
        );
    }

    #[test]
    fn nxdomain_is_unhealthy() {
        let server = fake_resolver(|query| {
            let mut response = query;
            response[2] = 0x81;
            response[3] = 0x83; // rcode 3
            response[6] = 0;
            response[7] = 0;
            response
        });
        let probe = probe_with(server, "missing.test", RecordType::A);
        let report = probe.probe(&resource());
        assert!(!report.succeeded);
        assert_eq!(report.status, HealthStatus::Unhealthy);
        assert!(
            report.observation.contains("NXDOMAIN"),
            "{}",
            report.observation
        );
    }

    #[test]
    fn no_answers_is_unhealthy() {
        let server = fake_resolver(|query| {
            let mut response = query;
            response[2] = 0x81;
            response[3] = 0x80;
            response[6] = 0;
            response[7] = 0;
            response
        });
        let probe = probe_with(server, "empty.test", RecordType::A);
        let report = probe.probe(&resource());
        assert!(!report.succeeded);
        assert_eq!(report.status, HealthStatus::Unhealthy);
        assert!(
            report.observation.contains("no A records"),
            "{}",
            report.observation
        );
    }

    #[test]
    fn silent_resolver_times_out() {
        // Bind and never answer.
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let server = socket.local_addr().unwrap();
        let probe = probe_with(server, "silent.test", RecordType::A);
        let started = std::time::Instant::now();
        let report = probe.probe(&resource());
        assert!(!report.succeeded);
        assert_eq!(report.status, HealthStatus::Unreachable);
        assert!(
            report.observation.contains("no response"),
            "{}",
            report.observation
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        drop(socket);
    }

    #[test]
    fn expect_regex_filters_answers() {
        let server = fake_resolver(answer_a([10, 0, 0, 7]));
        let mut probe = probe_with(server, "api.test", RecordType::A);
        probe.expect = Some(Regex::new(r"^10\.0\.0\.").unwrap());
        let report = probe.probe(&resource());
        assert!(report.succeeded, "{}", report.observation);

        let server = fake_resolver(answer_a([10, 0, 0, 7]));
        let mut probe = probe_with(server, "api.test", RecordType::A);
        probe.expect = Some(Regex::new(r"^192\.168\.").unwrap());
        let report = probe.probe(&resource());
        assert!(!report.succeeded);
        assert_eq!(report.status, HealthStatus::Unhealthy);
    }

    #[test]
    fn aaaa_record_is_parsed() {
        let server = fake_resolver(|query| {
            let mut response = query.clone();
            response[2] = 0x81;
            response[3] = 0x80;
            response[6] = 0;
            response[7] = 1;
            response.extend_from_slice(&[0xC0, 0x0C]);
            response.extend_from_slice(&TYPE_AAAA.to_be_bytes());
            response.extend_from_slice(&CLASS_IN.to_be_bytes());
            response.extend_from_slice(&60u32.to_be_bytes());
            response.extend_from_slice(&16u16.to_be_bytes());
            response
                .extend_from_slice(&[0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
            response
        });
        let probe = probe_with(server, "v6.test", RecordType::Aaaa);
        let report = probe.probe(&resource());
        assert!(report.succeeded, "{}", report.observation);
        assert!(
            report.observation.contains("2001:db8::1"),
            "{}",
            report.observation
        );
    }

    #[test]
    fn cname_answer_is_parsed() {
        let server = fake_resolver(|query| {
            let mut response = query.clone();
            response[2] = 0x81;
            response[3] = 0x80;
            response[6] = 0;
            response[7] = 1;
            response.extend_from_slice(&[0xC0, 0x0C]);
            response.extend_from_slice(&TYPE_CNAME.to_be_bytes());
            response.extend_from_slice(&CLASS_IN.to_be_bytes());
            response.extend_from_slice(&60u32.to_be_bytes());
            let target = b"\x05alias\x04test\x00";
            response.extend_from_slice(&u16::try_from(target.len()).unwrap().to_be_bytes());
            response.extend_from_slice(target);
            response
        });
        let probe = probe_with(server, "www.test", RecordType::Cname);
        let report = probe.probe(&resource());
        assert!(report.succeeded, "{}", report.observation);
        assert!(
            report.observation.contains("alias.test"),
            "{}",
            report.observation
        );
    }

    #[test]
    fn id_mismatch_is_rejected() {
        let mut probe = probe_with(
            UdpSocket::bind("127.0.0.1:0")
                .unwrap()
                .local_addr()
                .unwrap(),
            "x.test",
            RecordType::A,
        );
        probe.timeout = Duration::from_millis(200);
        let query = build_query("x.test", TYPE_A).unwrap();
        // Craft a response with a wrong id and parse it directly.
        let mut response = query.clone();
        response[0] ^= 0xFF;
        response[2] = 0x81;
        response[3] = 0x80;
        let id = u16::from_be_bytes([query[0], query[1]]);
        let err = parse_response(&response, id, TYPE_A).unwrap_err();
        assert!(err.contains("id mismatch"), "{err}");
    }

    #[test]
    fn compression_loops_do_not_hang() {
        // A name whose first label is a pointer to itself.
        let packet = [
            0x12, 0x34, 0x81, 0x80, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, // header, qd=1
            0xC0, 0x0C, // question name: pointer to itself
            0x00, 0x01, 0x00, 0x01,
        ];
        let err = parse_response(&packet, 0x1234, TYPE_A).unwrap_err();
        assert!(err.contains("loop"), "{err}");
    }

    #[test]
    fn config_requires_name_and_parses_server() {
        use serde_json::json;
        let probe =
            from_config(&json!({"name": "api.test", "server": "127.0.0.1", "record_type": "AAAA"}))
                .unwrap();
        assert_eq!(probe.record_type, RecordType::Aaaa);
        assert_eq!(probe.server.port(), 53);

        let probe = from_config(&json!({"name": "api.test", "server": "127.0.0.1:5353"})).unwrap();
        assert_eq!(probe.server.port(), 5353);

        assert!(from_config(&json!({"server": "127.0.0.1"})).is_err());
        assert!(from_config(&json!({"name": "x", "record_type": "TXT"})).is_err());
        assert!(from_config(&json!({"name": "x", "expect": "("})).is_err());
    }
}
