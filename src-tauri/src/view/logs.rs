//! Pod log streaming, batched for the UI.

use std::time::Duration;

use futures::{AsyncBufReadExt, io::BufReader};
use k8s_openapi::api::core::v1::Pod;
use kube::Api;
use kube::api::LogParams;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use super::Sink;
use crate::error::ErrorInfo;

const LOG_TICK: Duration = Duration::from_millis(50);
/// Upper bound per batch so one IPC message stays small even for floods.
const MAX_BATCH: usize = 2_000;
/// Lines longer than this are truncated (single JSON log lines can be MBs).
const MAX_LINE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogOptions {
    pub container: Option<String>,
    /// Lines of history to start with; `None` = all.
    pub tail_lines: Option<i64>,
    pub timestamps: bool,
    /// Logs of the previous (crashed) container instance.
    pub previous: bool,
    pub follow: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum LogEvent {
    Lines {
        lines: Vec<String>,
    },
    /// The stream finished (container exited, or `follow` was off).
    End,
    Error {
        error: ErrorInfo,
    },
}

pub async fn run_logs(api: Api<Pod>, pod: String, options: LogOptions, sink: Sink<LogEvent>) {
    let params = LogParams {
        container: options.container.clone(),
        follow: options.follow && !options.previous,
        tail_lines: options.tail_lines,
        timestamps: options.timestamps,
        previous: options.previous,
        ..Default::default()
    };
    let reader = match api.log_stream(&pod, &params).await {
        Ok(reader) => reader,
        Err(err) => {
            sink(LogEvent::Error {
                error: ErrorInfo::from_kube(&err),
            });
            return;
        }
    };

    let (tx, mut rx) = mpsc::channel::<Result<String, ErrorInfo>>(MAX_BATCH * 4);
    let reader_task = tokio::spawn(read_lines(BufReader::new(reader), tx));

    let mut tick = tokio::time::interval(LOG_TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut batch: Vec<String> = Vec::new();
    let mut open = true;

    while open {
        tokio::select! {
            line = rx.recv(), if batch.len() < MAX_BATCH => match line {
                Some(Ok(line)) => batch.push(line),
                Some(Err(error)) => {
                    flush(&sink, &mut batch);
                    sink(LogEvent::Error { error });
                    open = false;
                }
                None => {
                    flush(&sink, &mut batch);
                    sink(LogEvent::End);
                    open = false;
                }
            },
            _ = tick.tick() => {
                if !flush(&sink, &mut batch) {
                    open = false;
                }
            }
        }
    }
    reader_task.abort();
}

/// Sends the batch; returns false when the subscriber is gone.
fn flush(sink: &Sink<LogEvent>, batch: &mut Vec<String>) -> bool {
    if batch.is_empty() {
        return true;
    }
    sink(LogEvent::Lines {
        lines: std::mem::take(batch),
    })
}

/// Reads lines byte-wise so invalid UTF-8 is replaced instead of ending the stream.
async fn read_lines<R: futures::AsyncBufRead + Unpin>(
    mut reader: R,
    tx: mpsc::Sender<Result<String, ErrorInfo>>,
) {
    let mut buf = Vec::with_capacity(4096);
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) => return,
            Ok(_) => {
                if tx.send(Ok(decode_line(&buf))).await.is_err() {
                    return;
                }
            }
            Err(err) => {
                let _ = tx
                    .send(Err(ErrorInfo {
                        code: crate::error::ErrorCode::Api,
                        message: format!("log stream interrupted: {err}"),
                    }))
                    .await;
                return;
            }
        }
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_invalid_utf8_and_strips_newlines() {
        assert_eq!(decode_line(b"hello\r\n"), "hello");
        assert_eq!(decode_line(b"bad \xff byte\n"), "bad \u{fffd} byte");
    }

    #[test]
    fn truncates_huge_lines() {
        let raw = vec![b'a'; MAX_LINE_BYTES + 10];
        let line = decode_line(&raw);
        assert!(line.ends_with("[10 bytes truncated]"));
    }

    #[tokio::test]
    async fn reader_emits_every_line_including_last_without_newline() {
        let data: &[u8] = b"one\ntwo\n\xffthree";
        let (tx, mut rx) = mpsc::channel(16);
        read_lines(futures::io::Cursor::new(data), tx).await;
        let mut lines = Vec::new();
        while let Some(Ok(l)) = rx.recv().await {
            lines.push(l);
        }
        assert_eq!(lines, ["one", "two", "\u{fffd}three"]);
    }
}
