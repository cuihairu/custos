//! HTTP(S) endpoint probe (survey §26.2, implementation order #1).
//!
//! One `GET` covers: status-code whitelist (exact or `2xx` range), keyword
//! assertion, fail-if-matches regex, and a response-time threshold.
//! Redirects are opt-in (with a no-downgrade guard), TLS verification is
//! configurable for self-signed internal endpoints. Implemented on
//! std TCP + rustls — no shell, no external `curl`.

use crate::config::{bool_or, optional_str, required_str, string_array, u64_or};
use crate::{tls, Probe, ProbeReport};
use custos_model::{HealthStatus, Resource};
use regex::Regex;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

mod json;
mod net;

pub use self::json::{check_json_assertion, json_path, JsonAssertion, JsonOp};

use self::net::{
    is_timeout, parse_response, parse_url, resolve_location, response_complete, status_allowed,
    Connection, HttpResponse, Scheme,
};

const MAX_REDIRECTS: u32 = 5;

const DEFAULT_MAX_RESPONSE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct HttpProbe {
    pub url: String,
    pub timeout: Duration,
    pub allowed_statuses: Vec<String>,
    pub contains: Option<String>,
    pub fail_if_matches: Option<Regex>,
    pub response_time_threshold_ms: Option<u64>,
    pub follow_redirects: bool,
    pub max_redirects: u32,
    pub verify_tls: bool,
    pub max_response_bytes: usize,
    /// JSON assertions evaluated on the response body when it parses as JSON
    /// (probe-survey.md §26.2: JSON path / value comparison).
    pub json_assertions: Vec<JsonAssertion>,
}

pub fn from_config(config: &serde_json::Value) -> Result<HttpProbe, String> {
    let url = required_str(config, "url").ok_or("`probe.url` is required for http-endpoint")?;
    let fail_if_matches = match optional_str(config, "fail_if_matches") {
        Some(pattern) => Some(
            Regex::new(&pattern)
                .map_err(|err| format!("invalid `probe.fail_if_matches` regex: {err}"))?,
        ),
        None => None,
    };
    let mut json_assertions = Vec::new();
    if let Some(list) = config["json_assertions"].as_array() {
        for (index, entry) in list.iter().enumerate() {
            let path = required_str(entry, "path")
                .ok_or_else(|| format!("`probe.json_assertions[{index}].path` is required"))?;
            let op_text = entry["op"].as_str().unwrap_or("==");
            let op = JsonOp::from_str(op_text)
                .map_err(|err| format!("`probe.json_assertions[{index}]`: {err}"))?;
            let value = entry["value"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_default();
            if op != JsonOp::Exists && value.is_empty() {
                return Err(format!(
                    "`probe.json_assertions[{index}].value` is required for operator {op_text}"
                ));
            }
            json_assertions.push(JsonAssertion {
                path: path.to_owned(),
                op,
                value,
            });
        }
    }
    Ok(HttpProbe {
        url,
        timeout: Duration::from_millis(u64_or(config, "timeout_ms", 5_000)),
        allowed_statuses: string_array(config, "allowed_statuses"),
        contains: optional_str(config, "contains"),
        fail_if_matches,
        response_time_threshold_ms: {
            let ms = u64_or(config, "response_time_threshold_ms", 0);
            (ms > 0).then_some(ms)
        },
        follow_redirects: bool_or(config, "follow_redirects", false),
        max_redirects: {
            // Reject instead of `as u32`: a 4GiB-plus value silently
            // truncates to 0 (redirects off) or 1, lying about the config.
            let raw = u64_or(config, "max_redirects", u64::from(MAX_REDIRECTS));
            u32::try_from(raw)
                .map_err(|_| format!("`probe.max_redirects` {raw} does not fit in u32"))?
        },
        verify_tls: bool_or(config, "verify_tls", true),
        max_response_bytes: {
            let bytes = u64_or(config, "max_response_bytes", 0);
            if bytes > 0 {
                bytes as usize
            } else {
                DEFAULT_MAX_RESPONSE_BYTES
            }
        },
        json_assertions,
    })
}

impl Probe for HttpProbe {
    fn name(&self) -> &'static str {
        "http"
    }

    fn probe(&self, resource: &Resource) -> ProbeReport {
        let started = Instant::now();
        let report = |succeeded, status, observation: String| ProbeReport {
            resource_id: resource.id.clone(),
            probe_name: self.name().to_owned(),
            succeeded,
            status,
            observation,
        };
        let elapsed = || started.elapsed();

        let mut url = self.url.clone();
        let mut hops = 0;
        loop {
            let response = match self.fetch_once(&url, started) {
                Ok(response) => response,
                Err(err) => return report(false, HealthStatus::Unreachable, err),
            };
            let redirect = (300..400).contains(&response.status);
            if redirect && !self.follow_redirects {
                let target = response.location.clone().unwrap_or_default();
                return report(
                    false,
                    HealthStatus::Unhealthy,
                    format!(
                        "redirect to {target} not followed (status {})",
                        response.status
                    ),
                );
            }
            if redirect && hops < self.max_redirects {
                let Some(location) = response.location.clone() else {
                    return report(
                        false,
                        HealthStatus::Unreachable,
                        format!(
                            "redirect status {} without Location header",
                            response.status
                        ),
                    );
                };
                match resolve_location(&url, &location) {
                    // Never downgrade HTTPS to HTTP mid-chain; evaluate the
                    // redirect itself instead of following it blindly.
                    Ok(next) if url.starts_with("https://") && next.starts_with("http://") => {
                        return report(
                            false,
                            HealthStatus::Unhealthy,
                            format!("refusing https->http redirect to {next}"),
                        );
                    }
                    Ok(next) => {
                        url = next;
                        hops += 1;
                        continue;
                    }
                    Err(err) => {
                        return report(false, HealthStatus::Unreachable, err);
                    }
                }
            }
            if redirect {
                return report(
                    false,
                    HealthStatus::Unhealthy,
                    format!("too many redirects (> {})", self.max_redirects),
                );
            }
            return self.evaluate(&report, &url, response.status, &response.body, elapsed());
        }
    }
}

impl HttpProbe {
    fn fetch_once(&self, url: &str, started: Instant) -> Result<HttpResponse, String> {
        let parsed = parse_url(url)?;
        let remaining = |what: &str| -> Result<Duration, String> {
            let left = self.timeout.saturating_sub(started.elapsed());
            if left.is_zero() {
                return Err(format!(
                    "{what} timeout after {}ms",
                    self.timeout.as_millis()
                ));
            }
            Ok(left)
        };

        let addresses: Vec<std::net::SocketAddr> =
            ToSocketAddrs::to_socket_addrs(&(parsed.host.as_str(), parsed.port))
                .map_err(|err| format!("dns resolution failed for {}: {err}", parsed.host))?
                .collect();
        let address = addresses
            .into_iter()
            .next()
            .ok_or_else(|| format!("dns resolution returned no addresses for {}", parsed.host))?;
        let stream =
            TcpStream::connect_timeout(&address, remaining("connect")?).map_err(|err| {
                if err.kind() == std::io::ErrorKind::TimedOut {
                    format!("connect timeout after {}ms", self.timeout.as_millis())
                } else {
                    format!("connect to {address} failed: {err}")
                }
            })?;
        stream
            .set_read_timeout(Some(remaining("read")?))
            .map_err(|err| format!("set read timeout: {err}"))?;
        stream
            .set_write_timeout(Some(remaining("write")?))
            .map_err(|err| format!("set write timeout: {err}"))?;

        let mut connection = match parsed.scheme {
            Scheme::Http => Connection::Plain(stream),
            Scheme::Https => Connection::Tls(Box::new(tls::connect_tls(
                stream,
                &parsed.host,
                self.verify_tls,
            )?)),
        };

        let host_header = match (parsed.scheme, parsed.port) {
            (Scheme::Http, 80) | (Scheme::Https, 443) => parsed.host.clone(),
            (_, port) => format!("{}:{port}", parsed.host),
        };
        let request = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: custos-probe/1\r\nAccept: */*\r\nConnection: close\r\n\r\n",
            parsed.path, host_header
        );
        connection
            .write_all(request.as_bytes())
            .and_then(|()| connection.flush())
            .map_err(|err| {
                if is_timeout(&err) {
                    format!("write timeout after {}ms", self.timeout.as_millis())
                } else {
                    format!("write failed: {err}")
                }
            })?;

        let mut raw = Vec::with_capacity(8 * 1024);
        let mut buf = [0u8; 4096];
        loop {
            match connection.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let room = self.max_response_bytes.saturating_sub(raw.len());
                    raw.extend_from_slice(&buf[..n.min(room)]);
                    if raw.len() >= self.max_response_bytes {
                        break;
                    }
                    if response_complete(&raw) {
                        break;
                    }
                }
                Err(err) => {
                    // Keep-alive servers may hold the socket open after a
                    // complete response; only a timeout on an incomplete
                    // response is an error.
                    if raw.is_empty() {
                        if is_timeout(&err) {
                            return Err(format!(
                                "read timeout after {}ms",
                                self.timeout.as_millis()
                            ));
                        }
                        return Err(format!("read failed: {err}"));
                    }
                    break;
                }
            }
        }
        if raw.is_empty() {
            return Err("empty response".to_owned());
        }
        parse_response(&raw, self.max_response_bytes).map_err(|err| format!("bad response: {err}"))
    }

    /// Status whitelist + keyword/regex assertions + response-time threshold.
    fn evaluate(
        &self,
        report: &dyn Fn(bool, HealthStatus, String) -> ProbeReport,
        url: &str,
        status: u16,
        body: &str,
        elapsed: Duration,
    ) -> ProbeReport {
        let elapsed_ms = elapsed.as_millis();
        let problems: Vec<String> = {
            let mut problems = Vec::new();
            if !status_allowed(&self.allowed_statuses, status) {
                problems.push(format!("status {status} not in whitelist"));
            }
            if let Some(needle) = &self.contains {
                if !body.contains(needle.as_str()) {
                    problems.push(format!("body missing keyword {needle:?}"));
                }
            }
            if let Some(pattern) = &self.fail_if_matches {
                if pattern.is_match(body) {
                    problems.push(format!("body matches forbidden pattern {pattern}"));
                }
            }
            if !self.json_assertions.is_empty() {
                match serde_json::from_str::<serde_json::Value>(body) {
                    Ok(root) => {
                        for assertion in &self.json_assertions {
                            if let Some(problem) = check_json_assertion(&root, assertion) {
                                problems.push(problem);
                            }
                        }
                    }
                    Err(err) => problems.push(format!(
                        "body is not valid JSON ({} assertion(s) skipped): {err}",
                        self.json_assertions.len()
                    )),
                }
            }
            problems
        };
        let threshold = self.response_time_threshold_ms;
        if !problems.is_empty() {
            return report(
                false,
                HealthStatus::Unhealthy,
                format!(
                    "GET {url} -> status {status} in {elapsed_ms}ms, failed: {}",
                    problems.join("; ")
                ),
            );
        }
        if let Some(threshold) = threshold {
            if elapsed_ms > u128::from(threshold) {
                return report(
                    true,
                    HealthStatus::Degraded,
                    format!("GET {url} -> status {status} in {elapsed_ms}ms (slow, threshold {threshold}ms)"),
                );
            }
        }
        report(
            true,
            HealthStatus::Healthy,
            format!("GET {url} -> status {status} in {elapsed_ms}ms"),
        )
    }
}
#[cfg(test)]
mod tests;
