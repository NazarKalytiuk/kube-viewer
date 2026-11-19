//! Live subscriptions: table views, single-object detail and pod logs.
//!
//! Each subscription is a tokio task that pushes events through a sink
//! (a Tauri `Channel` in the app, a plain closure in tests). High-frequency
//! sources are coalesced on a fixed tick so the UI re-renders at most a few
//! times per second regardless of cluster churn.

pub mod detail;
pub mod logs;
pub mod store;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use kube::runtime::{WatchStreamExt, watcher};
use serde::Serialize;

use crate::error::{ErrorCode, ErrorInfo};
use crate::resources::rows::{RowSource, api_for, build_row};
use store::ViewStore;

/// How often a view publishes accumulated changes.
pub const VIEW_TICK: Duration = Duration::from_millis(100);

/// Callback that delivers an event to the subscriber; returns false once the
/// subscriber is gone, which ends the subscription task.
pub type Sink<T> = Arc<dyn Fn(T) -> bool + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ViewPhase {
    /// Initial list in progress; rows arrive progressively.
    Loading,
    Ready,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewUpdate {
    pub version: u64,
    /// Rows passing the filter.
    pub total: usize,
    /// All rows in the view.
    pub unfiltered: usize,
    pub phase: ViewPhase,
    /// Last watch error; the watcher keeps retrying with backoff.
    pub error: Option<ErrorInfo>,
}

pub fn watcher_error(err: &watcher::Error) -> ErrorInfo {
    match err {
        watcher::Error::InitialListFailed(e)
        | watcher::Error::WatchStartFailed(e)
        | watcher::Error::WatchFailed(e) => ErrorInfo::from_kube(e),
        watcher::Error::WatchError(status) => ErrorInfo {
            code: match status.code {
                401 => ErrorCode::Unauthorized,
                403 => ErrorCode::Forbidden,
                404 => ErrorCode::NotFound,
                _ => ErrorCode::Api,
            },
            message: status.message.clone(),
        },
        other => ErrorInfo {
            code: ErrorCode::Api,
            message: other.to_string(),
        },
    }
}

/// Watches all objects of `K` (optionally in one namespace) into `store`.
///
/// `kube::runtime::watcher` handles paging, resourceVersion bookkeeping,
/// bookmarks and 410 Gone re-lists; on every (re-)list the store content is
/// replaced atomically at `InitDone`, so objects deleted while the watch was
/// down disappear too.
pub async fn run_view<K: RowSource>(
    client: kube::Client,
    namespace: Option<String>,
    include_namespace: bool,
    store: Arc<Mutex<ViewStore>>,
    sink: Sink<ViewUpdate>,
) {
    let api = api_for::<K>(client, namespace.as_deref());
    let stream = watcher(api, watcher::Config::default().page_size(500)).default_backoff();
    futures::pin_mut!(stream);

    let mut tick = tokio::time::interval(VIEW_TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let mut phase = ViewPhase::Loading;
    let mut error: Option<ErrorInfo> = None;
    let mut relist: Option<Vec<crate::resources::Row>> = None;
    let mut first_list = true;
    let mut dirty = true;

    loop {
        tokio::select! {
            event = stream.next() => {
                let Some(event) = event else { break };
                dirty = true;
                match event {
                    Ok(event) => {
                        error = None;
                        apply_event::<K>(event, include_namespace, &store, &mut relist, &mut first_list, &mut phase);
                    }
                    Err(err) => error = Some(watcher_error(&err)),
                }
            }
            _ = tick.tick() => {
                if !dirty {
                    continue;
                }
                dirty = false;
                let update = {
                    let store = store.lock().expect("view store poisoned");
                    ViewUpdate {
                        version: store.version(),
                        total: store.total(),
                        unfiltered: store.unfiltered(),
                        phase,
                        error: error.clone(),
                    }
                };
                if !sink(update) {
                    break;
                }
            }
        }
    }
}

fn apply_event<K: RowSource>(
    event: watcher::Event<K>,
    include_namespace: bool,
    store: &Mutex<ViewStore>,
    relist: &mut Option<Vec<crate::resources::Row>>,
    first_list: &mut bool,
    phase: &mut ViewPhase,
) {
    match event {
        watcher::Event::Init => *relist = Some(Vec::new()),
        watcher::Event::InitApply(obj) => {
            if let Some(row) = build_row(&obj, include_namespace) {
                if *first_list {
                    // Show rows progressively while the first list is paging in.
                    store
                        .lock()
                        .expect("view store poisoned")
                        .upsert(row.clone());
                }
                relist.get_or_insert_with(Vec::new).push(row);
            }
        }
        watcher::Event::InitDone => {
            let rows = relist.take().unwrap_or_default();
            store.lock().expect("view store poisoned").replace_all(rows);
            *first_list = false;
            *phase = ViewPhase::Ready;
        }
        watcher::Event::Apply(obj) => {
            if let Some(row) = build_row(&obj, include_namespace) {
                store.lock().expect("view store poisoned").upsert(row);
            }
        }
        watcher::Event::Delete(obj) => {
            if let Some(uid) = obj.meta().uid.as_deref() {
                store.lock().expect("view store poisoned").remove(uid);
            }
        }
    }
}
