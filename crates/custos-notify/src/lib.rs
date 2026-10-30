//! Notification engine (docs/guide/workspace.md: `custos-notify`;
//! docs/guide/notification.md §8).
//!
//! The engine consumes `NotificationEvent`s (custos-model), matches them
//! against the independent `[notification]` rules, and dispatches to
//! configured channels. Delivery failures never block recovery (§8.1):
//! channels return a typed [`ChannelError`] and the queue engine decides
//! retry/backoff/dead-letter (§8.6).
//!
//! P1 scope: local log (base audit output), Webhook and SMTP channels, a
//! persisted queue that keeps working while the network is down.

pub mod channel;
pub mod engine;
pub mod herald;
pub mod local_log;
pub mod queue;
pub mod smtp;
pub mod webhook;
pub mod webhook_ssrf;

pub use channel::{Channel, ChannelError, ChannelErrorKind};
pub use engine::{DeliverReport, DispatchReport, NotificationEngine};
pub use herald::{dedup_key, dispatch_payload, herald_state, urgency, HeraldChannel};
pub use local_log::{format_event, severity_level, LocalLogChannel};
pub use queue::{NotificationQueue, QueueError, QueuedNotification};
pub use smtp::SmtpChannel;
pub use webhook::{sign_body, WebhookChannel, EVENT_ID_HEADER, SIGNATURE_HEADER};
pub use webhook_ssrf::{check_ip, validate_target, SsrfBlock};

/// Shared helpers for channel tests that talk to a real local HTTP socket.
#[cfg(test)]
pub(crate) mod test_http {
    use std::io::Read;
    use std::net::TcpStream;
    use std::time::Duration;

    /// Reads until the request body (per Content-Length) is fully received,
    /// so the captured buffer is never truncated and the responder never
    /// closes while the client is still writing.
    pub fn read_full_request(stream: &mut TcpStream) -> Vec<u8> {
        stream
            .set_read_timeout(Some(Duration::from_millis(2000)))
            .ok();
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        loop {
            if let Some(end) = find_header_end(&buf) {
                let total = end + parse_content_length(&buf[..end]);
                if buf.len() >= total {
                    return buf;
                }
            }
            match stream.read(&mut tmp) {
                Ok(0) => return buf,
                Ok(n) => buf.extend_from_slice(&tmp[..n]),
                Err(_) => return buf,
            }
        }
    }

    fn find_header_end(buf: &[u8]) -> Option<usize> {
        buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
    }

    fn parse_content_length(header: &[u8]) -> usize {
        let text = String::from_utf8_lossy(header).to_lowercase();
        text.lines()
            .find_map(|line| {
                line.trim_start()
                    .strip_prefix("content-length:")
                    .and_then(|v| v.trim().parse::<usize>().ok())
            })
            .unwrap_or(0)
    }
}
