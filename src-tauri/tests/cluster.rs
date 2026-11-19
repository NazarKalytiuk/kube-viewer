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
use kube_viewer_lib::logs::download::{DownloadEvent, DownloadRequest, run_download};
use kube_viewer_lib::logs::session::{LogPhase, LogRequest, LogStatus, run_log_session};
use kube_viewer_lib::logs::store::LogStore;
use kube_viewer_lib::logs::timefmt::TimeMode;
use kube_viewer_lib::resources::{ResourceKind, columns_for};
use kube_viewer_lib::view::detail::{DetailEvent, run_detail};
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
async fn logs_history_from_a_system_pod() {
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

    let store = Arc::new(Mutex::new(LogStore::default()));
    let (sink, mut rx) = channel_sink::<LogStatus>();
    let request = LogRequest {
        containers: vec!["coredns".into()],
        tail_lines: Some(5),
        since_seconds: None,
        previous: false,
        follow: false,
    };
    let task = tokio::spawn(run_log_session(pods, name, request, store.clone(), sink));
    let status = wait_for(&mut rx, |s| s.phase == LogPhase::Ended).await;
    assert_eq!(status.error, None);
    let s = store.lock().unwrap();
    let texts: Vec<String> = s
        .window(0, 100)
        .into_iter()
        .map(|(_, l)| l.text.to_string())
        .collect();
    assert_eq!(
        texts,
        expected.lines().map(str::to_string).collect::<Vec<_>>()
    );
    assert!(
        s.window(0, 100)
            .iter()
            .all(|(_, l)| l.ts != kube_viewer_lib::logs::timefmt::NO_TS)
    );
    drop(s);
    task.abort();
}

/// Pod with two containers: `app` logs 100 numbered lines per run and exits
/// (restartPolicy Always restarts it); `sidecar` logs continuously.
fn log_pod(namespace: &str) -> Pod {
    const IMAGE: &str = "docker.io/kindest/local-path-helper:v20260131-7181c60a";
    // The image has bash but no coreutils: sleep and cat are bash builtins here.
    const SLEEP: &str = r#"sleep() { read -rt "$1" <> <(:) || :; };"#;
    let app = format!(
        r#"{SLEEP} run=0; [ -f /data/run ] && run=$(</data/run); echo $((run+1)) > /data/run;
for ((i=0;i<100;i++)); do echo "app run=$run line=$i"; sleep 0.01; done; sleep 1"#
    );
    let sidecar = format!(
        r#"{SLEEP} i=0; while true; do echo "sidecar line=$i"; i=$((i+1)); sleep 0.05; done"#
    );
    serde_json::from_value(serde_json::json!({
        "metadata": { "name": "logger", "namespace": namespace },
        "spec": {
            "terminationGracePeriodSeconds": 0,
            "volumes": [{ "name": "data", "emptyDir": {} }],
            "containers": [
                { "name": "app", "image": IMAGE, "imagePullPolicy": "IfNotPresent",
                  "command": ["bash", "-c", app], "volumeMounts": [{ "name": "data", "mountPath": "/data" }] },
                { "name": "sidecar", "image": IMAGE, "imagePullPolicy": "IfNotPresent", "command": ["bash", "-c", sidecar] }
            ]
        }
    }))
    .unwrap()
}

async fn wait_running(pods: &Api<Pod>, name: &str) {
    tokio::time::timeout(TIMEOUT, async {
        loop {
            let pod = pods.get(name).await.unwrap();
            if pod.status.and_then(|s| s.phase).as_deref() == Some("Running") {
                return;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .expect("pod did not start");
}

fn numbers(lines: &[String], prefix: &str) -> Vec<u64> {
    lines
        .iter()
        .filter(|l| l.starts_with(prefix))
        .filter_map(|l| l.rsplit("line=").next()?.parse().ok())
        .collect()
}

#[tokio::test]
async fn logs_follow_merges_containers_and_survives_restarts() {
    if !enabled() {
        return;
    }
    let (_clusters, client) = connect().await;
    let ns = TestNamespaces::create(&client, "logs", 1).await;
    let pods: Api<Pod> = Api::namespaced(client.clone(), &ns.names[0]);
    pods.create(&PostParams::default(), &log_pod(&ns.names[0]))
        .await
        .unwrap();
    wait_running(&pods, "logger").await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let store = Arc::new(Mutex::new(LogStore::default()));
    let (sink, mut rx) = channel_sink::<LogStatus>();
    let request = LogRequest {
        containers: vec!["app".into(), "sidecar".into()],
        tail_lines: None,
        since_seconds: None,
        previous: false,
        follow: true,
    };
    let task = tokio::spawn(run_log_session(
        pods.clone(),
        "logger".into(),
        request,
        store.clone(),
        sink,
    ));
    wait_for(&mut rx, |s| s.phase == LogPhase::Following).await;

    // Wait until `app` has restarted at least once (first back-off is ~10 s).
    let app_runs = |store: &Mutex<LogStore>| {
        let s = store.lock().unwrap();
        s.window(0, usize::MAX)
            .iter()
            .filter(|(_, l)| l.container == 0 && l.text.contains("line=99"))
            .count()
    };
    tokio::time::timeout(Duration::from_secs(90), async {
        while app_runs(&store) < 2 {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    })
    .await
    .expect("app container did not restart");
    tokio::time::sleep(Duration::from_secs(1)).await;

    let texts: Vec<String> = {
        let s = store.lock().unwrap();
        let all = s.window(0, usize::MAX);
        // Backlog and live lines are ordered by time across containers.
        let backlog_end = all
            .iter()
            .position(|(_, l)| l.container == 1 && l.text.ends_with("line=40"))
            .unwrap_or(0);
        assert!(
            all[..backlog_end]
                .windows(2)
                .all(|w| w[0].1.ts <= w[1].1.ts),
            "backlog not merged by time"
        );
        all.iter().map(|(_, l)| l.text.to_string()).collect()
    };

    // Sidecar: one continuous run, no gaps or duplicates across reconnects.
    let sidecar = numbers(&texts, "sidecar");
    assert!(sidecar.len() > 50);
    assert!(
        sidecar.windows(2).all(|w| w[1] == w[0] + 1),
        "sidecar sequence broken: {sidecar:?}"
    );
    // App: each run complete exactly once (0..=99), across the restart.
    for run in 0..2 {
        let lines = numbers(&texts, &format!("app run={run} "));
        assert_eq!(lines, (0..100).collect::<Vec<_>>(), "run {run}");
    }
    task.abort();

    // Download all logs of both containers, merged and prefixed, in UTC.
    let path = std::env::temp_dir().join(format!("kv-logs-{}.log", std::process::id()));
    let (sink, mut rx) = channel_sink::<DownloadEvent>();
    let request = DownloadRequest {
        containers: vec!["app".into(), "sidecar".into()],
        previous: false,
        time_mode: TimeMode::Utc,
        container_prefix: true,
        path: path.display().to_string(),
    };
    tokio::spawn(run_download(pods.clone(), "logger".into(), request, sink));
    let done = wait_for(&mut rx, |e| !matches!(e, DownloadEvent::Progress { .. })).await;
    let DownloadEvent::Done { lines, .. } = done else {
        panic!("download failed: {done:?}")
    };
    let content = std::fs::read_to_string(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(content.lines().count() as u64, lines);
    let stamps: Vec<&str> = content
        .lines()
        .map(|l| l.split(' ').next().unwrap())
        .collect();
    assert!(
        stamps.windows(2).all(|w| w[0] <= w[1]),
        "export not ordered by time"
    );
    let re =
        regex::Regex::new(r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z \[(app|sidecar)\] ").unwrap();
    assert!(
        content.lines().all(|l| re.is_match(l)),
        "unexpected line format"
    );
    assert!(
        content.contains("[sidecar] sidecar line=0\n"),
        "sidecar history missing"
    );
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
