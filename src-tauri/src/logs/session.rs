//! A live log session: backlog first (merged by time across containers),
//! then one follower per container that resumes after container restarts
//! and dropped connections without duplicating lines.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::join_all;
use jiff::Timestamp;
use k8s_openapi::api::core::v1::Pod;
use kube::Api;
use kube::api::LogParams;
use serde::{Deserialize, Serialize};

use super::LineReader;
use super::store::{LogLine, LogStore};
use super::timefmt::{NO_TS, from_nanos};
use crate::error::{ErrorCode, ErrorInfo, kube_error_code};
use crate::view::Sink;

const STATUS_TICK: Duration = Duration::from_millis(100);
/// Lines are moved into the store in batches to keep lock traffic low.
const PUSH_BATCH: usize = 512;
const RECONNECT_MIN: Duration = Duration::from_secs(1);
const RECONNECT_MAX: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogRequest {
    /// One or more containers; several are merged by timestamp.
    pub containers: Vec<String>,
    /// Lines of history per container (`None` = all available).
    pub tail_lines: Option<i64>,
    /// Only history newer than this many seconds (`None` = no limit).
    pub since_seconds: Option<i64>,
    /// Logs of the previous (terminated) container instance.
    pub previous: bool,
    pub follow: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LogPhase {
    /// Fetching history.
    Loading,
    /// Streaming new lines.
    Following,
    /// The stream ended (container restarting, connection lost); retrying.
    Reconnecting,
    /// Finished (no follow, or the pod is gone).
    Ended,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogStatus {
    pub version: u64,
    /// Lines shown by the view (after filtering).
    pub visible: usize,
    /// Lines held in memory.
    pub lines: usize,
    pub dropped: u64,
    pub matches: usize,
    pub phase: LogPhase,
    pub error: Option<ErrorInfo>,
}

#[derive(Debug)]
struct Shared {
    phase: LogPhase,
    error: Option<ErrorInfo>,
    /// Followers currently attached to a stream.
    streaming: usize,
    /// Followers that gave up for good (pod deleted).
    finished: usize,
}

pub async fn run_log_session(
    api: Api<Pod>,
    pod: String,
    request: LogRequest,
    store: Arc<Mutex<LogStore>>,
    sink: Sink<LogStatus>,
) {
    let shared = Arc::new(Mutex::new(Shared {
        phase: LogPhase::Loading,
        error: None,
        streaming: 0,
        finished: 0,
    }));
    let worker = work(api, pod, request, store.clone(), shared.clone());
    futures::pin_mut!(worker);
    let mut worker_done = false;

    let mut tick = tokio::time::interval(STATUS_TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_sent: Option<LogStatus> = None;

    // The session outlives the worker so the view can keep reading the store
    // until the subscriber closes it.
    loop {
        tokio::select! {
            _ = &mut worker, if !worker_done => worker_done = true,
            _ = tick.tick() => {
                let status = snapshot(&store, &shared);
                if last_sent.as_ref() != Some(&status) {
                    if !sink(status.clone()) {
                        return;
                    }
                    last_sent = Some(status);
                }
            }
        }
    }
}

fn snapshot(store: &Mutex<LogStore>, shared: &Mutex<Shared>) -> LogStatus {
    let store = store.lock().expect("log store poisoned");
    let shared = shared.lock().expect("log state poisoned");
    LogStatus {
        version: store.version(),
        visible: store.visible_count(),
        lines: store.len(),
        dropped: store.dropped(),
        matches: store.match_count(),
        phase: shared.phase,
        error: shared.error.clone(),
    }
}

fn params(request: &LogRequest, container: &str) -> LogParams {
    LogParams {
        container: Some(container.to_string()),
        follow: false,
        timestamps: true,
        tail_lines: request.tail_lines,
        since_seconds: request.since_seconds,
        previous: request.previous,
        ..Default::default()
    }
}

async fn work(
    api: Api<Pod>,
    pod: String,
    request: LogRequest,
    store: Arc<Mutex<LogStore>>,
    shared: Arc<Mutex<Shared>>,
) {
    // Anything logged after this instant is guaranteed to reach a follower.
    let started = Timestamp::now();

    if request.containers.len() == 1 {
        // Stream straight into the store so long histories show up progressively.
        if let Err(error) = read_backlog(&api, &pod, &request, 0, |batch| {
            store.lock().expect("log store poisoned").extend(batch)
        })
        .await
        {
            set_error(&shared, Some(error));
        }
    } else {
        // Read all containers concurrently, then merge by time.
        let results = join_all(request.containers.iter().enumerate().map(|(i, _)| {
            let (api, pod, request) = (&api, &pod, &request);
            async move {
                let mut lines = Vec::new();
                let result =
                    read_backlog(api, pod, request, i as u16, |batch| lines.extend(batch)).await;
                (lines, result)
            }
        }))
        .await;
        let mut merged: Vec<LogLine> = Vec::new();
        for (lines, result) in results {
            merged.extend(lines);
            if let Err(error) = result {
                set_error(&shared, Some(error));
            }
        }
        // Stable sort: each container is already ordered; ties keep container order.
        merged.sort_by_key(|l| l.ts);
        store.lock().expect("log store poisoned").extend(merged);
    }

    if !request.follow || request.previous {
        shared.lock().expect("log state poisoned").phase = LogPhase::Ended;
        return;
    }

    shared.lock().expect("log state poisoned").phase = LogPhase::Reconnecting;
    let followers = request
        .containers
        .iter()
        .enumerate()
        .map(|(i, name)| follow(&api, &pod, name, i as u16, started, &store, &shared));
    join_all(followers).await;
    shared.lock().expect("log state poisoned").phase = LogPhase::Ended;
}

async fn read_backlog(
    api: &Api<Pod>,
    pod: &str,
    request: &LogRequest,
    index: u16,
    mut sink: impl FnMut(Vec<LogLine>),
) -> Result<(), ErrorInfo> {
    let container = &request.containers[index as usize];
    let reader = api
        .log_stream(pod, &params(request, container))
        .await
        .map_err(|e| container_error(container, &e))?;
    let mut reader = LineReader::new(reader);
    let mut batch = Vec::with_capacity(PUSH_BATCH);
    while let Some(line) = reader.next(index).await {
        match line {
            Ok(line) => {
                batch.push(line);
                if batch.len() == PUSH_BATCH {
                    sink(std::mem::replace(
                        &mut batch,
                        Vec::with_capacity(PUSH_BATCH),
                    ));
                }
            }
            Err(err) => {
                sink(batch);
                return Err(ErrorInfo {
                    code: ErrorCode::Api,
                    message: format!("{container}: log stream interrupted: {err}"),
                });
            }
        }
    }
    sink(batch);
    Ok(())
}

/// Follows one container forever: on every disconnect it resumes from the
/// newest timestamp it has stored, skipping lines it already has.
async fn follow(
    api: &Api<Pod>,
    pod: &str,
    container: &str,
    index: u16,
    started: Timestamp,
    store: &Mutex<LogStore>,
    shared: &Mutex<Shared>,
) {
    let mut backoff = RECONNECT_MIN;
    loop {
        let last_ts = store
            .lock()
            .expect("log store poisoned")
            .last_ts(index)
            .unwrap_or(NO_TS);
        // sinceTime has second precision; duplicates are dropped by timestamp below.
        let since = from_nanos(last_ts).unwrap_or(started);
        let params = LogParams {
            container: Some(container.to_string()),
            follow: true,
            timestamps: true,
            since_time: Some(since),
            ..Default::default()
        };
        match api.log_stream(pod, &params).await {
            Ok(reader) => {
                attach(shared, true);
                let mut reader = LineReader::new(reader);
                let mut received = false;
                while let Some(line) = reader.next(index).await {
                    let Ok(line) = line else { break };
                    if line.ts != NO_TS && last_ts != NO_TS && line.ts <= last_ts {
                        continue;
                    }
                    received = true;
                    store.lock().expect("log store poisoned").push(line);
                }
                attach(shared, false);
                if received {
                    backoff = RECONNECT_MIN;
                }
                if container_finished(api, pod, container).await {
                    shared.lock().expect("log state poisoned").finished += 1;
                    return;
                }
            }
            Err(err) if kube_error_code(&err) == ErrorCode::NotFound && is_pod_gone(&err) => {
                set_error(
                    shared,
                    Some(ErrorInfo {
                        code: ErrorCode::NotFound,
                        message: format!("pod {pod} no longer exists"),
                    }),
                );
                shared.lock().expect("log state poisoned").finished += 1;
                return;
            }
            Err(err) => set_error(shared, Some(container_error(container, &err))),
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(RECONNECT_MAX);
    }
}

/// True when the container will not produce more logs: the pod completed,
/// an init container finished, or the container terminated and its restart
/// policy never restarts it.
async fn container_finished(api: &Api<Pod>, pod: &str, container: &str) -> bool {
    let Ok(pod) = api.get(pod).await else {
        return false;
    };
    let Some(status) = pod.status else {
        return false;
    };
    if matches!(status.phase.as_deref(), Some("Succeeded" | "Failed")) {
        return true;
    }
    let terminated = |statuses: &Option<Vec<k8s_openapi::api::core::v1::ContainerStatus>>| {
        statuses.iter().flatten().any(|c| {
            c.name == container && c.state.as_ref().is_some_and(|s| s.terminated.is_some())
        })
    };
    // A terminated init container never runs again (native sidecars, which
    // restart, are running rather than terminated).
    if terminated(&status.init_container_statuses) {
        return true;
    }
    let never_restarts = pod.spec.and_then(|s| s.restart_policy).as_deref() == Some("Never");
    never_restarts && terminated(&status.container_statuses)
}

fn is_pod_gone(err: &kube::Error) -> bool {
    matches!(err, kube::Error::Api(status) if status.message.contains("not found") && !status.message.contains("container"))
}

fn attach(shared: &Mutex<Shared>, attached: bool) {
    let mut s = shared.lock().expect("log state poisoned");
    if attached {
        s.streaming += 1;
        s.error = None;
    } else {
        s.streaming = s.streaming.saturating_sub(1);
    }
    s.phase = if s.streaming > 0 {
        LogPhase::Following
    } else {
        LogPhase::Reconnecting
    };
}

fn set_error(shared: &Mutex<Shared>, error: Option<ErrorInfo>) {
    shared.lock().expect("log state poisoned").error = error;
}

fn container_error(container: &str, err: &kube::Error) -> ErrorInfo {
    let mut info = ErrorInfo::from_kube(err);
    info.message = format!("{container}: {}", info.message);
    info
}
