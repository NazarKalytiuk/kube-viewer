//! Pod logs: live sessions backed by a Rust-side store, search, and export.

pub mod download;
pub mod merge;
pub mod session;
pub mod store;
pub mod timefmt;

use futures::AsyncBufReadExt;

use store::LogLine;
use timefmt::{TimeFormatter, split_timestamp};

use crate::error::{ErrorCode, ErrorInfo};

/// Lines longer than this are truncated (single JSON log lines can be MBs).
pub const MAX_LINE_BYTES: usize = 64 * 1024;

/// Reads one log line at a time, tolerating invalid UTF-8.
pub struct LineReader<R> {
    reader: R,
    buf: Vec<u8>,
}

impl<R: futures::AsyncBufRead + Unpin> LineReader<R> {
    pub fn new(reader: R) -> Self {
        LineReader {
            reader,
            buf: Vec::with_capacity(4096),
        }
    }

    /// Next line parsed into (timestamp, text); `None` at end of stream.
    pub async fn next(&mut self, container: u16) -> Option<std::io::Result<LogLine>> {
        self.buf.clear();
        match self.reader.read_until(b'\n', &mut self.buf).await {
            Ok(0) => None,
            Ok(_) => {
                let raw = decode_line(&self.buf);
                let (ts, text) = split_timestamp(&raw);
                Some(Ok(LogLine {
                    ts,
                    container,
                    text: text.into(),
                }))
            }
            Err(err) => Some(Err(err)),
        }
    }
}

/// A container's log stream broke off while being read.
pub fn stream_interrupted(container: &str, err: &std::io::Error) -> ErrorInfo {
    ErrorInfo {
        code: ErrorCode::Api,
        message: format!("{container}: log stream interrupted: {err}"),
    }
}

/// Lossy UTF-8 decode, trailing newline removed, oversized lines truncated.
pub fn decode_line(raw: &[u8]) -> String {
    let raw = raw.strip_suffix(b"\n").unwrap_or(raw);
    let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
    if raw.len() > MAX_LINE_BYTES {
        let mut line = String::from_utf8_lossy(&raw[..MAX_LINE_BYTES]).into_owned();
        line.push_str(&format!(
            " … [{} bytes truncated]",
            raw.len() - MAX_LINE_BYTES
        ));
        line
    } else {
        String::from_utf8_lossy(raw).into_owned()
    }
}

/// Upper bound for copying to the clipboard; larger ranges should be downloaded.
pub const MAX_COPY_LINES: usize = 200_000;

/// Text of visible lines `[start, end]` (inclusive) exactly as exported:
/// time per `fmt`, container tag when several containers are shown.
pub fn render_range(
    store: &store::LogStore,
    start: usize,
    end: usize,
    fmt: &TimeFormatter,
    containers: &[String],
) -> (String, usize) {
    let prefix = containers.len() > 1;
    let mut out = String::new();
    let lines = store.window(start, end.saturating_add(1));
    for (_, line) in &lines {
        let container = prefix
            .then(|| containers.get(line.container as usize).map(String::as_str))
            .flatten();
        format_export_line(&mut out, fmt, line, container);
    }
    // No trailing newline: pasting a single line should not add one.
    if out.ends_with('\n') {
        out.pop();
    }
    (out, lines.len())
}

/// One exported line: `[time ][[container] ]text\n`.
pub fn format_export_line(
    out: &mut String,
    fmt: &TimeFormatter,
    line: &LogLine,
    container: Option<&str>,
) {
    if let Some(ts) = fmt.format(line.ts) {
        out.push_str(&ts);
        out.push(' ');
    }
    if let Some(c) = container {
        out.push('[');
        out.push_str(c);
        out.push_str("] ");
    }
    out.push_str(&line.text);
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;
    use timefmt::{NO_TS, TimeMode};

    #[test]
    fn decodes_invalid_utf8_and_strips_newlines() {
        assert_eq!(decode_line(b"hello\r\n"), "hello");
        assert_eq!(decode_line(b"bad \xff byte\n"), "bad \u{fffd} byte");
    }

    #[test]
    fn truncates_huge_lines() {
        let raw = vec![b'a'; MAX_LINE_BYTES + 10];
        assert!(decode_line(&raw).ends_with("[10 bytes truncated]"));
    }

    #[tokio::test]
    async fn reader_parses_timestamps_and_last_line_without_newline() {
        let data: &[u8] = b"2026-11-19T10:00:00.5Z one\n2026-11-19T10:00:01Z two\n\xffthree";
        let mut r = LineReader::new(futures::io::Cursor::new(data));
        let mut out = Vec::new();
        while let Some(line) = r.next(3).await {
            out.push(line.unwrap());
        }
        assert_eq!(
            out.iter().map(|l| &*l.text).collect::<Vec<_>>(),
            ["one", "two", "\u{fffd}three"]
        );
        assert!(out[0].ts < out[1].ts);
        assert_eq!(out[2].ts, NO_TS);
        assert!(out.iter().all(|l| l.container == 3));
    }

    #[test]
    fn render_range_is_inclusive_and_tags_containers() {
        let mut s = store::LogStore::default();
        for (i, c) in [(0, 0u16), (1, 1), (2, 0), (3, 1)] {
            s.push(LogLine {
                ts: NO_TS,
                container: c,
                text: format!("l{i}").into(),
            });
        }
        let fmt = TimeFormatter::new(TimeMode::None);
        let two = vec!["app".to_string(), "db".to_string()];
        assert_eq!(
            render_range(&s, 1, 2, &fmt, &two),
            ("[db] l1\n[app] l2".to_string(), 2)
        );
        assert_eq!(
            render_range(&s, 3, 3, &fmt, &two[..1]),
            ("l3".to_string(), 1)
        );
        assert_eq!(
            render_range(&s, 2, 99, &fmt, &two[..1]).1,
            2,
            "clamped to the end"
        );
    }

    #[test]
    fn export_line_formats() {
        let (ts, _) = split_timestamp("2026-11-19T10:00:00.5Z x");
        let line = LogLine {
            ts,
            container: 0,
            text: "hello".into(),
        };
        let mut out = String::new();
        format_export_line(
            &mut out,
            &TimeFormatter::new(TimeMode::Utc),
            &line,
            Some("app"),
        );
        format_export_line(&mut out, &TimeFormatter::new(TimeMode::None), &line, None);
        assert_eq!(out, "2026-11-19T10:00:00.500Z [app] hello\nhello\n");
    }
}
