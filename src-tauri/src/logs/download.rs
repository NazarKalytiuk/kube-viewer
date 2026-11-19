//! Exporting logs to a file.
//!
//! "All logs" streams the complete log of each container straight from the
//! API to disk (constant memory, k-way merged by timestamp across
//! containers). "Loaded lines" writes what the view currently holds,
//! honouring its filter. Both write to a temporary file that is renamed on
//! success, so a cancelled or failed export never leaves a partial file.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use k8s_openapi::api::core::v1::Pod;
use kube::Api;
use kube::api::LogParams;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use super::store::{LogLine, LogStore};
use super::timefmt::{TimeFormatter, TimeMode};
use super::{LineReader, format_export_line};
use crate::error::{AppError, AppResult, ErrorCode, ErrorInfo};
use crate::view::Sink;

const PROGRESS_EVERY: Duration = Duration::from_millis(200);
const WRITE_CHUNK: usize = 256 * 1024;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadRequest {
    pub containers: Vec<String>,
    pub previous: bool,
    pub time_mode: TimeMode,
    /// Prefix lines with `[container]` (useful when exporting several).
    pub container_prefix: bool,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum DownloadEvent {
    Progress {
        lines: u64,
        bytes: u64,
    },
    Done {
        path: String,
        lines: u64,
        bytes: u64,
    },
    Error {
        error: ErrorInfo,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveResult {
    pub path: String,
    pub lines: u64,
    pub bytes: u64,
}

fn partial_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".partial");
    path.with_file_name(name)
}

fn io_error(path: &Path, err: std::io::Error) -> ErrorInfo {
    ErrorInfo {
        code: ErrorCode::Internal,
        message: format!("{}: {err}", path.display()),
    }
}

/// Removes the temporary file if the export did not finish (error or cancel).
struct PartialGuard(Option<PathBuf>);

impl Drop for PartialGuard {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

pub async fn run_download(
    api: Api<Pod>,
    pod: String,
    request: DownloadRequest,
    sink: Sink<DownloadEvent>,
) {
    let event = match download(&api, &pod, &request, &sink).await {
        Ok((lines, bytes)) => DownloadEvent::Done {
            path: request.path.clone(),
            lines,
            bytes,
        },
        Err(error) => DownloadEvent::Error { error },
    };
    sink(event);
}

async fn download(
    api: &Api<Pod>,
    pod: &str,
    request: &DownloadRequest,
    sink: &Sink<DownloadEvent>,
) -> Result<(u64, u64), ErrorInfo> {
    if request.containers.is_empty() {
        return Err(ErrorInfo {
            code: ErrorCode::InvalidRequest,
            message: "no container selected".into(),
        });
    }
    let target = PathBuf::from(&request.path);
    let partial = partial_path(&target);
    let mut guard = PartialGuard(Some(partial.clone()));
    let file = tokio::fs::File::create(&partial)
        .await
        .map_err(|e| io_error(&partial, e))?;
    let mut writer = tokio::io::BufWriter::with_capacity(WRITE_CHUNK, file);

    let mut readers = Vec::with_capacity(request.containers.len());
    for container in &request.containers {
        let params = LogParams {
            container: Some(container.clone()),
            timestamps: true,
            previous: request.previous,
            ..Default::default()
        };
        let reader = api.log_stream(pod, &params).await.map_err(|e| {
            let mut info = ErrorInfo::from_kube(&e);
            info.message = format!("{container}: {}", info.message);
            info
        })?;
        readers.push(LineReader::new(reader));
    }

    let fmt = TimeFormatter::new(request.time_mode);
    let prefix = request.container_prefix;
    let (mut lines, mut bytes) = (0u64, 0u64);
    let mut out = String::with_capacity(WRITE_CHUNK);
    let mut last_progress = Instant::now();

    // k-way merge: keep the next line of every container, always write the oldest.
    let mut heads: Vec<Option<LogLine>> = Vec::with_capacity(readers.len());
    for (i, reader) in readers.iter_mut().enumerate() {
        heads.push(next_line(reader, i as u16, &request.containers).await?);
    }
    while let Some(i) = oldest(&heads) {
        let line = heads[i].take().expect("oldest head exists");
        let container = prefix.then(|| request.containers[i].as_str());
        format_export_line(&mut out, &fmt, &line, container);
        lines += 1;
        heads[i] = next_line(&mut readers[i], i as u16, &request.containers).await?;

        if out.len() >= WRITE_CHUNK {
            bytes += out.len() as u64;
            writer
                .write_all(out.as_bytes())
                .await
                .map_err(|e| io_error(&partial, e))?;
            out.clear();
        }
        if last_progress.elapsed() >= PROGRESS_EVERY {
            last_progress = Instant::now();
            if !sink(DownloadEvent::Progress { lines, bytes }) {
                return Err(ErrorInfo {
                    code: ErrorCode::Internal,
                    message: "cancelled".into(),
                });
            }
        }
    }
    bytes += out.len() as u64;
    writer
        .write_all(out.as_bytes())
        .await
        .map_err(|e| io_error(&partial, e))?;
    writer.flush().await.map_err(|e| io_error(&partial, e))?;
    drop(writer);
    tokio::fs::rename(&partial, &target)
        .await
        .map_err(|e| io_error(&target, e))?;
    guard.0 = None;
    Ok((lines, bytes))
}

async fn next_line<R: futures::AsyncBufRead + Unpin>(
    reader: &mut LineReader<R>,
    index: u16,
    containers: &[String],
) -> Result<Option<LogLine>, ErrorInfo> {
    match reader.next(index).await {
        None => Ok(None),
        Some(Ok(line)) => Ok(Some(line)),
        Some(Err(err)) => Err(ErrorInfo {
            code: ErrorCode::Api,
            message: format!(
                "{}: log stream interrupted: {err}",
                containers[index as usize]
            ),
        }),
    }
}

/// Index of the head with the smallest timestamp (ties: lowest index, so
/// container order is stable).
fn oldest(heads: &[Option<LogLine>]) -> Option<usize> {
    heads
        .iter()
        .enumerate()
        .filter_map(|(i, h)| h.as_ref().map(|l| (l.ts, i)))
        .min()
        .map(|(_, i)| i)
}

/// Writes the lines currently visible in a log view (respecting its filter).
/// The store is locked only per chunk, so live streaming keeps flowing.
pub fn save_loaded(
    store: &Mutex<LogStore>,
    containers: &[String],
    time_mode: TimeMode,
    container_prefix: bool,
    path: &Path,
) -> AppResult<SaveResult> {
    use std::io::Write;

    let partial = partial_path(path);
    let mut guard = PartialGuard(Some(partial.clone()));
    let file = std::fs::File::create(&partial)
        .map_err(|e| AppError::Internal(format!("{}: {e}", partial.display())))?;
    let mut writer = std::io::BufWriter::with_capacity(WRITE_CHUNK, file);
    let fmt = TimeFormatter::new(time_mode);

    // Snapshot the line numbers, then copy in chunks so live streaming is
    // never blocked for long; lines evicted meanwhile are skipped.
    let numbers = store.lock().expect("log store poisoned").visible_numbers();
    let (mut lines, mut bytes) = (0u64, 0u64);
    let mut out = String::with_capacity(WRITE_CHUNK);
    for chunk in numbers.chunks(20_000) {
        {
            let store = store.lock().expect("log store poisoned");
            for &number in chunk {
                let Some(line) = store.line_by_number(number) else {
                    continue;
                };
                let container = container_prefix
                    .then(|| containers.get(line.container as usize).map(String::as_str))
                    .flatten();
                format_export_line(&mut out, &fmt, line, container);
                lines += 1;
            }
        }
        bytes += out.len() as u64;
        writer
            .write_all(out.as_bytes())
            .map_err(|e| AppError::Internal(format!("{}: {e}", partial.display())))?;
        out.clear();
    }
    writer
        .flush()
        .map_err(|e| AppError::Internal(format!("{}: {e}", partial.display())))?;
    drop(writer);
    std::fs::rename(&partial, path)
        .map_err(|e| AppError::Internal(format!("{}: {e}", path.display())))?;
    guard.0 = None;
    Ok(SaveResult {
        path: path.display().to_string(),
        lines,
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logs::store::{LogQuery, QueryMode};

    fn line(ts: i64, container: u16, text: &str) -> LogLine {
        LogLine {
            ts,
            container,
            text: text.into(),
        }
    }

    #[test]
    fn oldest_picks_smallest_timestamp_with_stable_ties() {
        let heads = vec![
            Some(line(5, 0, "a")),
            None,
            Some(line(3, 2, "c")),
            Some(line(3, 3, "d")),
        ];
        assert_eq!(oldest(&heads), Some(2));
        assert_eq!(oldest(&[None, None]), None);
    }

    #[test]
    fn saves_filtered_lines_with_prefix_atomically() {
        let dir = std::env::temp_dir().join(format!("kv-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("out.log");

        let store = Mutex::new(LogStore::default());
        {
            let mut s = store.lock().unwrap();
            s.extend([
                line(0, 0, "ok"),
                line(0, 1, "ERROR x"),
                line(0, 0, "error y"),
            ]);
            s.set_query(Some(&LogQuery {
                text: "error".into(),
                regex: false,
                case_sensitive: false,
                mode: QueryMode::Filter,
            }))
            .unwrap();
        }
        let containers = vec!["app".to_string(), "sidecar".to_string()];
        let result = save_loaded(&store, &containers, TimeMode::None, true, &path).unwrap();
        assert_eq!(result.lines, 2);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[sidecar] ERROR x\n[app] error y\n"
        );
        assert!(!partial_path(&path).exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
