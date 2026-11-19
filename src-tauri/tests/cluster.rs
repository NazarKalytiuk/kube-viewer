//! Integration tests against a real cluster.
//!
//! Opt-in: `KUBEVIEWER_IT=1 cargo test --test cluster -- --test-threads=1`.
//! Uses `KUBEVIEWER_IT_CONTEXT` (default `kind-kubeviewer-test`) and only
//! touches namespaces it creates (`kv-it-*`), deleting them afterwards.
//! The perf test is additionally `#[ignore]`d:
//! `KUBEVIEWER_IT=1 cargo test --release --test cluster -- --ignored --nocapture`.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::{StreamExt, TryStreamExt};
use k8s_openapi::api::core::v1::{ConfigMap, Namespace, Pod};
use kube::api::{DeleteParams, ListParams, LogParams, ObjectMeta, PostParams};
use kube::{Api, Client};
use tokio::sync::mpsc;

use kube_viewer_lib::cluster::Clusters;
use kube_viewer_lib::resources::{ResourceKind, columns_for};
use kube_viewer_lib::view::detail::{DetailEvent, run_detail};
use kube_viewer_lib::view::logs::{LogEvent, LogOptions, run_logs};
use kube_viewer_lib::view::store::ViewStore;
use kube_viewer_lib::view::{Sink, ViewPhase, ViewUpdate, run_view};

const TIMEOUT: Duration = Duration::from_secs(60);

fn enabled() -> bool {
    std::env::var("KUBEVIEWER_IT").is_ok_and(|v| v == "1")
}

fn context() -> String {
    std::env::var("KUBEVIEWER_IT_CONTEXT").unwrap_or_else(|_| "kind-kubeviewer-test".into())
}

async fn connect() -> (Clusters, Client) {
    let clusters = Clusters::default();
    let info = clusters
        .connect(&context())
        .await
        .expect("connect to test cluster");
    assert!(!info.server_version.is_empty());
    assert!(!info.namespaces.forbidden);
    let client = clusters.client(&context()).await.unwrap();
    (clusters, client)
}

/// Namespaces created by a test, deleted on drop even if the test fails.
struct TestNamespaces {
    names: Vec<String>,
}

impl TestNamespaces {
    async fn create(client: &Client, prefix: &str, count: usize) -> Self {
        let api: Api<Namespace> = Api::all(client.clone());
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let suffix = format!("{}-{}", std::process::id(), nanos % 100_000);
        let mut names = Vec::new();
        for i in 0..count {
            let name = format!("kv-it-{prefix}-{suffix}-{i}");
            let ns = Namespace {
                metadata: ObjectMeta {
                    name: Some(name.clone()),
                    ..Default::default()
                },
                ..Default::default()
            };
            api.create(&PostParams::default(), &ns)
                .await
                .expect("create namespace");
            names.push(name);
        }
        TestNamespaces { names }
    }
}

impl Drop for TestNamespaces {
    fn drop(&mut self) {
        let names = std::mem::take(&mut self.names);
        // Drop runs inside the (blocked) test runtime, whose client connections
        // cannot make progress; clean up with a fresh client on its own runtime.
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let options = kube::config::KubeConfigOptions {
                    context: Some(context()),
                    ..Default::default()
                };
                let config = kube::Config::from_kubeconfig(&options).await.unwrap();
                let api: Api<Namespace> = Api::all(Client::try_from(config).unwrap());
                for name in names {
                    let _ = api.delete(&name, &DeleteParams::background()).await;
                }
            });
        })
        .join()
        .unwrap();
    }
}

fn config_map(namespace: &str, name: &str) -> ConfigMap {
    ConfigMap {
        metadata: ObjectMeta {
            name: Some(name.into()),
            namespace: Some(namespace.into()),
            ..Default::default()
        },
        data: Some(BTreeMap::from([("key".to_string(), "value".to_string())])),
        ..Default::default()
    }
}

fn channel_sink<T: Send + 'static>() -> (Sink<T>, mpsc::UnboundedReceiver<T>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (Arc::new(move |event| tx.send(event).is_ok()), rx)
}

async fn wait_for<T: std::fmt::Debug>(
    rx: &mut mpsc::UnboundedReceiver<T>,
    mut pred: impl FnMut(&T) -> bool,
) -> T {
    tokio::time::timeout(TIMEOUT, async {
        loop {
            let event = rx.recv().await.expect("subscription ended");
            if pred(&event) {
                return event;
            }
        }
    })
    .await
    .expect("timed out waiting for event")
}

fn names_in(store: &Mutex<ViewStore>) -> Vec<(String, String)> {
    store
        .lock()
        .unwrap()
        .window(0, usize::MAX)
        .into_iter()
        .map(|r| (r.namespace.unwrap_or_default(), r.name))
        .collect()
}

#[tokio::test]
async fn view_tracks_same_name_across_namespaces_and_deletes() {
    if !enabled() {
        return;
    }
    let (_clusters, client) = connect().await;
    let ns = TestNamespaces::create(&client, "view", 2).await;
    for n in &ns.names {
        Api::<ConfigMap>::namespaced(client.clone(), n)
            .create(&PostParams::default(), &config_map(n, "shared"))
            .await
            .unwrap();
    }

    let store = Arc::new(Mutex::new(ViewStore::new(
        columns_for(ResourceKind::ConfigMaps, true),
        ResourceKind::ConfigMaps.default_sort(),
        "kv-it-view",
    )));
    let (sink, mut rx) = channel_sink::<ViewUpdate>();
    let task = tokio::spawn(run_view::<ConfigMap>(
        client.clone(),
        None,
        true,
        store.clone(),
        sink,
    ));

    // Each new namespace also gets a `kube-root-ca.crt` ConfigMap: two pairs
    // of identically named objects that must stay distinct rows.
    wait_for(&mut rx, |u| u.phase == ViewPhase::Ready && u.total == 4).await;
    let rows = names_in(&store);
    assert_eq!(rows.iter().filter(|(_, n)| n == "shared").count(), 2);
    assert_eq!(
        rows.iter().filter(|(_, n)| n == "kube-root-ca.crt").count(),
        2
    );

    Api::<ConfigMap>::namespaced(client.clone(), &ns.names[0])
        .delete("shared", &DeleteParams::default())
        .await
        .unwrap();
    wait_for(&mut rx, |u| u.total == 3).await;
    let shared: Vec<_> = names_in(&store)
        .into_iter()
        .filter(|(_, n)| n == "shared")
        .collect();
    assert_eq!(shared, [(ns.names[1].clone(), "shared".to_string())]);
    task.abort();
}

#[tokio::test]
async fn detail_reports_snapshot_then_deletion() {
    if !enabled() {
        return;
    }
    let (_clusters, client) = connect().await;
    let ns = TestNamespaces::create(&client, "detail", 1).await;
    let api = Api::<ConfigMap>::namespaced(client.clone(), &ns.names[0]);
    api.create(&PostParams::default(), &config_map(&ns.names[0], "watched"))
        .await
        .unwrap();

    let (sink, mut rx) = channel_sink::<DetailEvent>();
    let task = tokio::spawn(run_detail::<ConfigMap>(
        client.clone(),
        Some(ns.names[0].clone()),
        "watched".into(),
        sink,
    ));
    let snapshot = wait_for(&mut rx, |e| matches!(e, DetailEvent::Snapshot { .. })).await;
    let DetailEvent::Snapshot { detail } = snapshot else {
        unreachable!()
    };
    assert_eq!(detail.name, "watched");
    assert!(detail.yaml.contains("key: value"), "{}", detail.yaml);

    api.delete("watched", &DeleteParams::default())
        .await
        .unwrap();
    wait_for(&mut rx, |e| matches!(e, DetailEvent::Deleted)).await;
    task.abort();
}

#[tokio::test]
async fn logs_stream_from_a_system_pod() {
    if !enabled() {
        return;
    }
    let (_clusters, client) = connect().await;
    let pods: Api<Pod> = Api::namespaced(client.clone(), "kube-system");
    let pod = pods
        .list(&ListParams::default().labels("k8s-app=kube-dns"))
        .await
        .unwrap()
        .items
        .into_iter()
        .next()
        .expect("coredns pod");
    let name = pod.metadata.name.unwrap();
    // Sanity: the API has logs for this pod.
    let expected = pods
        .logs(
            &name,
            &LogParams {
                tail_lines: Some(5),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let (sink, mut rx) = channel_sink::<LogEvent>();
    let options = LogOptions {
        container: None,
        tail_lines: Some(5),
        timestamps: false,
        previous: false,
        follow: false,
    };
    let task = tokio::spawn(run_logs(pods, name, options, sink));
    let mut lines = Vec::new();
    loop {
        match wait_for(&mut rx, |_| true).await {
            LogEvent::Lines { lines: batch } => lines.extend(batch),
            LogEvent::End => break,
            LogEvent::Error { error } => panic!("log error: {error:?}"),
        }
    }
    assert_eq!(
        lines,
        expected.lines().map(str::to_string).collect::<Vec<_>>()
    );
    task.abort();
}

/// Loads a view with many objects and reports timings.
#[tokio::test]
#[ignore = "perf: creates thousands of objects"]
async fn perf_large_view() {
    if !enabled() {
        return;
    }
    let count: usize = std::env::var("KUBEVIEWER_PERF_COUNT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10_000);
    let (_clusters, client) = connect().await;
    let ns = TestNamespaces::create(&client, "perf", 1).await;
    let api = Api::<ConfigMap>::namespaced(client.clone(), &ns.names[0]);

    let created = Instant::now();
    futures::stream::iter(0..count)
        .map(|i| {
            let api = api.clone();
            let ns = ns.names[0].clone();
            async move {
                api.create(&PostParams::default(), &config_map(&ns, &format!("cm-{i}")))
                    .await
                    .map(|_| ())
            }
        })
        .buffer_unordered(64)
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    println!("created {count} config maps in {:?}", created.elapsed());

    let store = Arc::new(Mutex::new(ViewStore::new(
        columns_for(ResourceKind::ConfigMaps, false),
        ResourceKind::ConfigMaps.default_sort(),
        "",
    )));
    let (sink, mut rx) = channel_sink::<ViewUpdate>();
    let started = Instant::now();
    let task = tokio::spawn(run_view::<ConfigMap>(
        client.clone(),
        Some(ns.names[0].clone()),
        false,
        store.clone(),
        sink,
    ));
    let mut first_rows = None;
    let ready = wait_for(&mut rx, |u| {
        if first_rows.is_none() && u.total > 0 {
            first_rows = Some(started.elapsed());
        }
        u.phase == ViewPhase::Ready
    })
    .await;
    println!(
        "view: first rows after {:?}, ready with {} rows after {:?}",
        first_rows.unwrap_or_default(),
        ready.total,
        started.elapsed()
    );
    // +1: the namespace's auto-created `kube-root-ca.crt`.
    assert_eq!(ready.total, count + 1);

    let s = store.lock().unwrap();
    let t = Instant::now();
    for start in (0..count).step_by(count / 100) {
        assert_eq!(s.window(start, start + 60).len(), 60.min(count - start));
    }
    println!("100 window reads (60 rows each): {:?}", t.elapsed());
    drop(s);

    let mut s = store.lock().unwrap();
    let t = Instant::now();
    s.set_query(
        kube_viewer_lib::resources::SortSpec {
            column: "age".into(),
            descending: true,
        },
        "cm-99",
    );
    println!(
        "re-sort + filter of {count} rows: {:?} → {} rows",
        t.elapsed(),
        s.total()
    );
    drop(s);
    task.abort();
}
