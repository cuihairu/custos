//! Wire limits for the stdio protocol (docs/guide/plugins.md §4.4).

/// Default per-message cap: 1 MiB, configurable per host. Messages above
/// the cap are rejected with `MESSAGE_TOO_LARGE`; large evidence must be
/// summarized, truncated or chunked instead.
pub const DEFAULT_MAX_MESSAGE_BYTES: usize = 1024 * 1024;

/// Upper bound for stderr diagnostic buffering: the reader keeps draining
/// (so a chatty plugin can never block on a full pipe) but only retains
/// this much for incident reports.
pub const MAX_STDERR_RETAINED_BYTES: usize = 64 * 1024;

/// Compile-time check: stderr retention must stay below the message cap.
const _: () = assert!(MAX_STDERR_RETAINED_BYTES < DEFAULT_MAX_MESSAGE_BYTES);

/// Reads one newline-delimited line from a stdio reader, capping the
/// accumulation at `cap` bytes (§4.4). A line that grows past the cap (a
/// misbehaving host) is drained to its newline and returned as `None`, so
/// memory stays bounded instead of growing with the input; a clean EOF
/// returns `Ok(None)` and an unterminated final line returns it as-is.
pub fn read_capped_line<R: std::io::BufRead>(
    reader: &mut R,
    cap: usize,
) -> std::io::Result<Option<Vec<u8>>> {
    let mut line: Vec<u8> = Vec::new();
    let mut oversized = false;
    loop {
        let (newline_at, available) = {
            let buf = match reader.fill_buf() {
                Ok(buf) => buf,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            if buf.is_empty() {
                // EOF. Clean end-of-input with nothing pending is None;
                // an unterminated final line (or oversized drain) is Some.
                return Ok(if line.is_empty() && !oversized {
                    None
                } else {
                    Some(line)
                });
            }
            (buf.iter().position(|&b| b == b'\n'), buf.len())
        };
        let take = newline_at.map_or(available, |at| at + 1);
        if !oversized {
            if line.len() + take > cap {
                oversized = true;
                line.clear();
            } else {
                line.extend_from_slice(&reader.fill_buf()?[..take]);
            }
        }
        reader.consume(take);
        if newline_at.is_some() {
            return Ok(Some(line));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn read_all(input: &[u8], cap: usize) -> Vec<Option<Vec<u8>>> {
        let mut cursor = Cursor::new(input);
        let mut out = Vec::new();
        while let Ok(Some(line)) = read_capped_line(&mut cursor, cap) {
            out.push(Some(line));
        }
        out
    }

    #[test]
    fn reads_lines_within_the_cap() {
        let lines = read_all(b"one\ntwo\nthree", 100);
        assert_eq!(
            lines,
            vec![
                Some(b"one\n".to_vec()),
                Some(b"two\n".to_vec()),
                Some(b"three".to_vec()),
            ]
        );
    }

    #[test]
    fn oversized_line_is_drained_and_skipped() {
        let lines = read_all(b"aaaaaaaaaaaaaaaa\nnext\n", 8);
        // The 16-byte line exceeds the cap: drained (returns empty), the
        // following line is still read intact.
        assert_eq!(lines, vec![Some(Vec::new()), Some(b"next\n".to_vec())]);
    }
}
