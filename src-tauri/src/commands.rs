//! Tauri command surface. Commands are thin: validation + wiring of the
//! Tauri `Channel` into the transport-agnostic subscription tasks.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use futures::FutureExt;
use k8s_openapi::api::core::v1::{Pod, Secret};
use kube::Api;
use serde::Serialize;
use tauri::State;
use tauri::ipc::Channel;

use crate::cluster::{ClusterInfo, Clusters, NamespaceList};
use crate::error::{AppError, AppResult};
use crate::kubeconfig::{self, ContextList};
use crate::logs::download::{
    DownloadEvent, DownloadRequest, SaveResult, run_download, save_loaded,
};
use crate::logs::session::{LogRequest, LogStatus, run_log_session};
use crate::logs::store::{LogQuery, LogStore};
use crate::logs::timefmt::{TimeFormatter, TimeMode};
use crate::resources::{Column, ResourceKind, Row, SortSpec, columns_for};
use crate::shell_env::login_env;
use crate::subscriptions::{Attachment, Subscriptions};
use crate::view::detail::{DetailEvent, run_detail};
use crate::view::store::ViewStore;
use crate::view::{Sink, ViewUpdate, run_view};
use crate::with_kind;

/// Largest window the frontend may request at once.
const MAX_WINDOW: usize = 1_000;

#[derive(Default)]
pub struct AppState {
    pub clusters: Clusters,
    pub subscriptions: Arc<Subscriptions>,
}

fn sink<T: tauri::ipc::IpcResponse + Clone + Send + Sync + 'static>(
    channel: Channel<T>,
) -> Sink<T> {
    Arc::new(move |event: T| channel.send(event).is_ok())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KindInfo {
    kind: ResourceKind,
    title: &'static str,
    group: &'static str,
    namespaced: bool,
}

#[tauri::command]
pub fn resource_kinds() -> Vec<KindInfo> {
    ResourceKind::ALL
        .iter()
        .map(|&kind| KindInfo {
            kind,
            title: kind.title(),
            group: kind.group(),
            namespaced: kind.namespaced(),
        })
        .collect()
}

#[tauri::command]
pub async fn list_contexts() -> AppResult<ContextList> {
    let login = login_env().await;
    let (cfg, sources) = kubeconfig::load(login)?;
    Ok(kubeconfig::list_contexts(&cfg, &sources))
}

#[tauri::command]
pub async fn connect(state: State<'_, AppState>, context: String) -> AppResult<ClusterInfo> {
    // Subscriptions bound to the previous client of this context are stale.
    state.subscriptions.close_context(&context);
    let result = state.clusters.connect(&context).await;
    match &result {
        Ok(info) => tracing::info!(context, version = info.server_version, "connected"),
        Err(err) => tracing::warn!(context, %err, "connect failed"),
    }
    result
}

#[tauri::command]
pub async fn list_namespaces(
    state: State<'_, AppState>,
    context: String,
) -> AppResult<NamespaceList> {
    state.clusters.namespaces(&context).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewOpened {
    id: u64,
    columns: Vec<Column>,
    sort: SortSpec,
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn open_view(
    state: State<'_, AppState>,
    context: String,
    kind: ResourceKind,
    namespace: Option<String>,
    sort: Option<SortSpec>,
    filter: Option<String>,
    on_update: Channel<ViewUpdate>,
) -> AppResult<ViewOpened> {
    let client = state.clusters.client(&context).await?;
    let namespace = if kind.namespaced() {
        namespace.filter(|n| !n.is_empty())
    } else {
        None
    };
    let all_namespaces = kind.namespaced() && namespace.is_none();
    let columns = columns_for(kind, all_namespaces);
    let store = ViewStore::new(
        columns.clone(),
        sort.unwrap_or_else(|| kind.default_sort()),
        filter.as_deref().unwrap_or(""),
    );
    let sort = store.sort().clone();
    let store = Arc::new(Mutex::new(store));

    let task = with_kind!(kind, T => run_view::<T>(client, namespace, all_namespaces, store.clone(), sink(on_update)).boxed());
    let id = state
        .subscriptions
        .spawn(&context, Attachment::Table(store), task);
    tracing::debug!(id, ?kind, all_namespaces, "view opened");
    Ok(ViewOpened { id, columns, sort })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryResult {
    version: u64,
    total: usize,
    sort: SortSpec,
}

#[tauri::command]
pub fn set_view_query(
    state: State<'_, AppState>,
    id: u64,
    sort: SortSpec,
    filter: String,
) -> AppResult<QueryResult> {
    let view = state.subscriptions.view(id).ok_or_else(|| unknown(id))?;
    let mut store = view.lock().expect("view store poisoned");
    store.set_query(sort, &filter);
    Ok(QueryResult {
        version: store.version(),
        total: store.total(),
        sort: store.sort().clone(),
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RowsWindow {
    version: u64,
    total: usize,
    start: usize,
    rows: Vec<Row>,
}

#[tauri::command]
pub fn get_rows(
    state: State<'_, AppState>,
    id: u64,
    start: usize,
    end: usize,
) -> AppResult<RowsWindow> {
    if end < start || end - start > MAX_WINDOW {
        return Err(AppError::InvalidRequest(format!(
            "window [{start}, {end}) exceeds {MAX_WINDOW} rows"
        )));
    }
    let view = state.subscriptions.view(id).ok_or_else(|| unknown(id))?;
    let store = view.lock().expect("view store poisoned");
    Ok(RowsWindow {
        version: store.version(),
        total: store.total(),
        start,
        rows: store.window(start, end),
    })
}

#[tauri::command]
pub fn row_index(state: State<'_, AppState>, id: u64, uid: String) -> AppResult<Option<usize>> {
    let view = state.subscriptions.view(id).ok_or_else(|| unknown(id))?;
    Ok(view.lock().expect("view store poisoned").index_of(&uid))
}

#[tauri::command]
pub async fn open_detail(
    state: State<'_, AppState>,
    context: String,
    kind: ResourceKind,
    namespace: Option<String>,
    name: String,
    on_event: Channel<DetailEvent>,
) -> AppResult<u64> {
    let client = state.clusters.client(&context).await?;
    let namespace = if kind.namespaced() {
        Some(
            namespace
                .filter(|n| !n.is_empty())
                .ok_or_else(|| AppError::InvalidRequest("namespace is required".into()))?,
        )
    } else {
        None
    };
    let task =
        with_kind!(kind, T => run_detail::<T>(client, namespace, name, sink(on_event)).boxed());
    Ok(state.subscriptions.spawn(&context, Attachment::None, task))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogOpened {
    id: u64,
    containers: Vec<String>,
}

#[tauri::command]
pub async fn open_logs(
    state: State<'_, AppState>,
    context: String,
    namespace: String,
    pod: String,
    request: LogRequest,
    on_status: Channel<LogStatus>,
) -> AppResult<LogOpened> {
    if request.containers.is_empty() || request.containers.len() > u16::MAX as usize {
        return Err(AppError::InvalidRequest(
            "select at least one container".into(),
        ));
    }
    let client = state.clusters.client(&context).await?;
    let api: Api<Pod> = Api::namespaced(client, &namespace);
    let store = Arc::new(Mutex::new(LogStore::default()));
    let containers = Arc::new(request.containers.clone());
    let task = run_log_session(api, pod, request, store.clone(), sink(on_status)).boxed();
    let id = state.subscriptions.spawn(
        &context,
        Attachment::Logs {
            store,
            containers: containers.clone(),
        },
        task,
    );
    Ok(LogOpened {
        id,
        containers: containers.to_vec(),
    })
}

/// A log session's data, looked up by subscription id.
struct LogSession {
    store: Arc<Mutex<LogStore>>,
    containers: Arc<Vec<String>>,
}

fn log_session(state: &AppState, id: u64) -> AppResult<LogSession> {
    match state.subscriptions.attachment(id) {
        Attachment::Logs { store, containers } => Ok(LogSession { store, containers }),
        _ => Err(AppError::InvalidRequest(format!("unknown log view {id}"))),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogQueryResult {
    version: u64,
    visible: usize,
    matches: usize,
}

#[tauri::command]
pub fn set_log_query(
    state: State<'_, AppState>,
    id: u64,
    query: Option<LogQuery>,
) -> AppResult<LogQueryResult> {
    let store = log_session(&state, id)?.store;
    let mut store = store.lock().expect("log store poisoned");
    store.set_query(query.as_ref())?;
    Ok(LogQueryResult {
        version: store.version(),
        visible: store.visible_count(),
        matches: store.match_count(),
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WireLogLine {
    /// Line number in the session (stable while scrolling and filtering).
    n: u64,
    /// Formatted per the requested time mode; absent when hidden or unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    ts: Option<String>,
    /// Container index (into `LogOpened.containers`).
    c: u16,
    text: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogWindow {
    version: u64,
    total: usize,
    start: usize,
    lines: Vec<WireLogLine>,
}

#[tauri::command]
pub fn get_log_lines(
    state: State<'_, AppState>,
    id: u64,
    start: usize,
    end: usize,
    time_mode: TimeMode,
) -> AppResult<LogWindow> {
    if end < start || end - start > MAX_WINDOW {
        return Err(AppError::InvalidRequest(format!(
            "window [{start}, {end}) exceeds {MAX_WINDOW} lines"
        )));
    }
    let store = log_session(&state, id)?.store;
    let fmt = TimeFormatter::new(time_mode);
    let store = store.lock().expect("log store poisoned");
    let lines = store
        .window(start, end)
        .into_iter()
        .map(|(n, line)| WireLogLine {
            n,
            ts: fmt.format(line.ts),
            c: line.container,
            text: line.text.to_string(),
        })
        .collect();
    Ok(LogWindow {
        version: store.version(),
        total: store.visible_count(),
        start,
        lines,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FindResult {
    index: usize,
    ordinal: usize,
    matches: usize,
}

#[tauri::command]
pub fn find_log_match(
    state: State<'_, AppState>,
    id: u64,
    from: Option<usize>,
    forward: bool,
) -> AppResult<Option<FindResult>> {
    let store = log_session(&state, id)?.store;
    let store = store.lock().expect("log store poisoned");
    Ok(store
        .find(from, forward)
        .map(|(index, ordinal)| FindResult {
            index,
            ordinal,
            matches: store.match_count(),
        }))
}

#[tauri::command]
pub async fn save_loaded_logs(
    state: State<'_, AppState>,
    id: u64,
    path: String,
    time_mode: TimeMode,
    container_prefix: bool,
) -> AppResult<SaveResult> {
    let LogSession { store, containers } = log_session(&state, id)?;
    tokio::task::spawn_blocking(move || {
        save_loaded(
            &store,
            &containers,
            time_mode,
            container_prefix,
            std::path::Path::new(&path),
        )
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?
}

#[tauri::command]
pub async fn download_logs(
    state: State<'_, AppState>,
    context: String,
    namespace: String,
    pod: String,
    request: DownloadRequest,
    on_event: Channel<DownloadEvent>,
) -> AppResult<u64> {
    let client = state.clusters.client(&context).await?;
    let api: Api<Pod> = Api::namespaced(client, &namespace);
    let task = run_download(api, pod, request, sink(on_event)).boxed();
    Ok(state.subscriptions.spawn(&context, Attachment::None, task))
}

/// Suggested export path: `~/Downloads/<name>`, made unique if it exists.
#[tauri::command]
pub fn default_log_path(file_name: String) -> AppResult<String> {
    let safe: String = file_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let home =
        std::env::var_os("HOME").ok_or_else(|| AppError::Internal("HOME is not set".into()))?;
    let dir = std::path::PathBuf::from(home).join("Downloads");
    let (stem, ext) = match safe.rsplit_once('.') {
        Some((stem, ext)) => (stem.to_string(), format!(".{ext}")),
        None => (safe.clone(), String::new()),
    };
    let mut candidate = dir.join(&safe);
    let mut n = 1;
    while candidate.exists() {
        candidate = dir.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    Ok(candidate.display().to_string())
}

#[tauri::command]
pub fn close_subscription(state: State<'_, AppState>, id: u64) {
    state.subscriptions.close(id);
}

/// Called by the frontend on startup so a webview reload never leaves
/// orphaned watches running.
#[tauri::command]
pub fn reset_subscriptions(state: State<'_, AppState>) {
    state.subscriptions.close_all();
}

/// Decoded Secret values, fetched only on an explicit user action.
#[tauri::command]
pub async fn reveal_secret(
    state: State<'_, AppState>,
    context: String,
    namespace: String,
    name: String,
) -> AppResult<BTreeMap<String, String>> {
    let client = state.clusters.client(&context).await?;
    let secret = Api::<Secret>::namespaced(client, &namespace)
        .get(&name)
        .await?;
    Ok(secret
        .data
        .unwrap_or_default()
        .into_iter()
        .map(|(key, bytes)| {
            let value = match String::from_utf8(bytes.0) {
                Ok(text) => text,
                Err(err) => format!("<binary data, {} bytes>", err.as_bytes().len()),
            };
            (key, value)
        })
        .collect())
}

fn unknown(id: u64) -> AppError {
    AppError::InvalidRequest(format!("unknown view {id}"))
}
