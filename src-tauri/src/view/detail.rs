//! Live detail for one object, watched with a `metadata.name` field selector.

use std::time::Duration;

use futures::StreamExt;
use kube::runtime::{WatchStreamExt, watcher};
use serde::Serialize;

use super::{Sink, watcher_error};
use crate::error::ErrorInfo;
use crate::resources::detail::{Detail, DetailSource, build_detail};
use crate::resources::rows::api_for;

/// Objects such as Nodes update every few seconds; detail pushes are
/// coalesced to this interval.
const DETAIL_TICK: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum DetailEvent {
    Snapshot {
        detail: Box<Detail>,
    },
    /// The object no longer exists.
    Deleted,
    Error {
        error: ErrorInfo,
    },
}

pub async fn run_detail<K: DetailSource>(
    client: kube::Client,
    namespace: Option<String>,
    name: String,
    sink: Sink<DetailEvent>,
) {
    let api = api_for::<K>(client, namespace.as_deref());
    let config = watcher::Config::default().fields(&format!("metadata.name={name}"));
    let stream = watcher(api, config).default_backoff();
    futures::pin_mut!(stream);

    let mut tick = tokio::time::interval(DETAIL_TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut pending: Option<DetailEvent> = None;
    let mut seen_in_list = false;

    loop {
        tokio::select! {
            event = stream.next() => {
                let Some(event) = event else { break };
                match event {
                    Ok(watcher::Event::Init) => seen_in_list = false,
                    Ok(watcher::Event::Apply(obj) | watcher::Event::InitApply(obj)) => {
                        seen_in_list = true;
                        pending = Some(snapshot(&obj));
                    }
                    Ok(watcher::Event::InitDone) if !seen_in_list => pending = Some(DetailEvent::Deleted),
                    Ok(watcher::Event::InitDone) => {}
                    Ok(watcher::Event::Delete(_)) => pending = Some(DetailEvent::Deleted),
                    Err(err) => pending = Some(DetailEvent::Error { error: watcher_error(&err) }),
                }
            }
            _ = tick.tick() => {
                if let Some(event) = pending.take() && !sink(event) {
                    break;
                }
            }
        }
    }
}

fn snapshot<K: DetailSource>(obj: &K) -> DetailEvent {
    match build_detail(obj) {
        Ok(detail) => DetailEvent::Snapshot {
            detail: Box::new(detail),
        },
        Err(err) => DetailEvent::Error {
            error: (&err).into(),
        },
    }
}
