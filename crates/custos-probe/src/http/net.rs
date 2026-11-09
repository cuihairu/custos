//! URL parsing, connection setup (plain TCP / TLS) and response
//! parsing for the HTTP probe.

use crate::tls;
use std::io::{Read, Write};
use std::net::TcpStream;

pub(super) enum Connection {
    Plain(TcpStream),
    Tls(Box<tls::TlsStream>),
}

impl Read for Connection {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Connection::Plain(stream) => stream.read(buf),
            Connection::Tls(stream) => stream.read(buf),
        }
    }
}

impl Write for Connection {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Connection::Plain(stream) => stream.write(buf),
            Connection::Tls(stream) => stream.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Connection::Plain(stream) => stream.flush(),
            Connection::Tls(stream) => stream.flush(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Scheme {
    Http,
    Https,
}

#[derive(Debug, Clone)]
pub(super) struct ParsedUrl {
    pub(super) scheme: Scheme,
    pub(super) host: String,
    pub(super) port: u16,
    pub(super) path: String,
}

pub(super) fn parse_url(url: &str) -> Result<ParsedUrl, String> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| format!("url `{url}` has no scheme"))?;
    let scheme = match scheme {
        "http" => Scheme::Http,
        "https" => Scheme::Https,
        other => return Err(format!("unsupported url scheme `{other}`")),
    };
    let (authority, path) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, "/"),
    };
    let (host, port) = if let Some(stripped) = authority.strip_prefix('[') {
        // IPv6 literal: [::1]:443
        let (host, tail) = stripped
            .split_once(']')
            .ok_or_else(|| format!("unterminated IPv6 literal in `{url}`"))?;
        let port = tail
            .strip_prefix(':')
            .map(|p| p.parse::<u16>().map_err(|_| format!("bad port in `{url}`")))
            .transpose()?
            .unwrap_or(default_port(scheme));
        (host.to_owned(), port)
    } else if let Some((host, port)) = authority.rsplit_once(':') {
        let port = port
            .parse::<u16>()
            .map_err(|_| format!("bad port in `{url}`"))?;
        (host.to_owned(), port)
    } else {
        (authority.to_owned(), default_port(scheme))
    };
    if host.is_empty() {
        return Err(format!("url `{url}` has no host"));
    }
    Ok(ParsedUrl {
        scheme,
        host,
        port,
        path: path.to_owned(),
    })
}

pub(super) fn default_port(scheme: Scheme) -> u16 {
    match scheme {
        Scheme::Http => 80,
        Scheme::Https => 443,
    }
}

pub(super) fn resolve_location(current: &str, location: &str) -> Result<String, String> {
    if location.contains("://") {
        return Ok(location.to_owned());
    }
    let base = parse_url(current)?;
    let scheme = match base.scheme {
        Scheme::Http => "http",
        Scheme::Https => "https",
    };
    let authority = if (base.scheme, base.port) == (Scheme::Http, 80)
        || (base.scheme, base.port) == (Scheme::Https, 443)
    {
        base.host.clone()
    } else {
        format!("{}:{}", base.host, base.port)
    };
    if let Some(path) = location.strip_prefix('/') {
        Ok(format!("{scheme}://{authority}{path}"))
    } else {
        let dir = match base.path.rsplit_once('/') {
            Some((dir, _)) => dir,
            None => "",
        };
        Ok(format!("{scheme}://{authority}{dir}/{location}"))
    }
}

pub(super) fn status_allowed(allowed: &[String], status: u16) -> bool {
    if allowed.is_empty() {
        return (200..300).contains(&status);
    }
    allowed.iter().any(|pattern| {
        let pattern = pattern.trim();
        if let Some(class) = pattern.strip_suffix("xx") {
            if let Ok(class) = class.parse::<u16>() {
                return (1..=9).contains(&class) && status / 100 == class;
            }
        }
        pattern.parse::<u16>().is_ok_and(|exact| exact == status)
    })
}

pub(super) struct HttpResponse {
    pub(super) status: u16,
    pub(super) body: String,
    pub(super) location: Option<String>,
}

pub(super) fn is_timeout(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    )
}

pub(super) fn response_complete(raw: &[u8]) -> bool {
    let Some(headers_end) = find_headers_end(raw) else {
        return false;
    };
    let headers = String::from_utf8_lossy(&raw[..headers_end]).to_ascii_lowercase();
    let body = &raw[headers_end + 4..];
    if headers.contains("transfer-encoding:") && headers.contains("chunked") {
        return chunked_complete(body);
    }
    for line in headers.lines() {
        if let Some(value) = line.strip_prefix("content-length:") {
            if let Ok(len) = value.trim().parse::<usize>() {
                return body.len() >= len;
            }
        }
    }
    false
}

pub(super) fn find_headers_end(raw: &[u8]) -> Option<usize> {
    raw.windows(4).position(|window| window == b"\r\n\r\n")
}

pub(super) fn chunked_complete(body: &[u8]) -> bool {
    let mut offset = 0;
    loop {
        let rest = &body[offset..];
        let Some(size_end) = rest.windows(2).position(|w| w == b"\r\n") else {
            return false;
        };
        let size_text = std::str::from_utf8(&rest[..size_end]).ok().unwrap_or("");
        let size_text = size_text.split(';').next().unwrap_or("").trim();
        let Ok(size) = usize::from_str_radix(size_text, 16) else {
            return false;
        };
        // Same overflow guard as dechunk(): a declared size that wraps
        // `chunk_end` past usize::MAX is malformed, not complete.
        let Some(chunk_end) = (size_end + 2).checked_add(size) else {
            return false;
        };
        let Some(limit) = chunk_end.checked_add(2) else {
            return false;
        };
        if limit > rest.len() {
            return false;
        }
        if size == 0 {
            return true;
        }
        offset += chunk_end + 2;
    }
}

pub(super) fn parse_response(raw: &[u8], max_bytes: usize) -> Result<HttpResponse, String> {
    let headers_end = find_headers_end(raw).ok_or("no header terminator")?;
    let headers = std::str::from_utf8(&raw[..headers_end]).map_err(|_| "headers are not utf-8")?;
    let status_line = headers.lines().next().ok_or("empty response")?;
    let mut parts = status_line.split_whitespace();
    let version = parts.next().ok_or("malformed status line")?;
    if !version.starts_with("HTTP/") {
        return Err(format!("malformed status line {status_line:?}"));
    }
    let status: u16 = parts
        .next()
        .ok_or("status line without status code")?
        .parse()
        .map_err(|_| "status line without status code")?;
    let location = headers.lines().skip(1).find_map(|line| {
        line.split_once(':').and_then(|(name, value)| {
            name.trim()
                .eq_ignore_ascii_case("location")
                .then(|| value.trim().to_owned())
        })
    });
    let body_bytes = &raw[headers_end + 4..];
    let body = if headers.to_ascii_lowercase().contains("transfer-encoding:")
        && headers.to_ascii_lowercase().contains("chunked")
    {
        dechunk(body_bytes).unwrap_or_else(|| body_bytes.to_vec())
    } else {
        body_bytes.to_vec()
    };
    let body = String::from_utf8_lossy(&body);
    // Never keep more than the cap in memory; the cap was already applied
    // while reading, so this only guards the lossy conversion.
    let body: String = body.chars().take(max_bytes).collect();
    Ok(HttpResponse {
        status,
        body,
        location,
    })
}

pub(super) fn dechunk(body: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(body.len());
    let mut offset = 0;
    loop {
        let rest = body.get(offset..)?;
        let size_end = rest.windows(2).position(|w| w == b"\r\n")?;
        let size_text = std::str::from_utf8(&rest[..size_end]).ok()?;
        let size = usize::from_str_radix(size_text.split(';').next()?.trim(), 16).ok()?;
        // A peer can declare a huge chunk size (up to usize::MAX); the
        // unchecked `size_end + 2 + size` overflows (debug: panic, release:
        // wraps past the `rest.len()` guard) and the slice below then
        // indexes start > end. Use checked arithmetic so an oversized or
        // wrapping chunk is treated as a malformed body, not a crash.
        let data_start = size_end + 2;
        let chunk_end = data_start.checked_add(size)?;
        if chunk_end.checked_add(2)? > rest.len() {
            return None;
        }
        out.extend_from_slice(&rest[data_start..chunk_end]);
        if size == 0 {
            return Some(out);
        }
        offset += chunk_end + 2;
    }
}
