//! Typed object → table row conversion, mirroring `kubectl get` output.

use std::collections::BTreeMap;
use std::fmt::Debug;

use k8s_openapi::api::apps::v1::{DaemonSet, Deployment, ReplicaSet, StatefulSet};
use k8s_openapi::api::core::v1::{
    ConfigMap, ContainerStatus, Event, Namespace, Node, PersistentVolumeClaim, Pod, Secret, Service,
};
use k8s_openapi::api::networking::v1::Ingress;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;
use kube::ResourceExt;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::{Cell, Row, Tone};

/// Builds an [`kube::Api`] for a resource scope, so generic code can work with
/// both namespaced and cluster-scoped kinds.
pub trait ApiScope: Sized {
    const NAMESPACED: bool;
    fn api<K: kube::Resource<Scope = Self, DynamicType = ()>>(
        client: kube::Client,
        namespace: Option<&str>,
    ) -> kube::Api<K>;
}

impl ApiScope for k8s_openapi::NamespaceResourceScope {
    const NAMESPACED: bool = true;
    fn api<K: kube::Resource<Scope = Self, DynamicType = ()>>(
        client: kube::Client,
        namespace: Option<&str>,
    ) -> kube::Api<K> {
        match namespace {
            Some(ns) => kube::Api::namespaced(client, ns),
            None => kube::Api::all(client),
        }
    }
}

impl ApiScope for k8s_openapi::ClusterResourceScope {
    const NAMESPACED: bool = false;
    fn api<K: kube::Resource<Scope = Self, DynamicType = ()>>(
        client: kube::Client,
        _namespace: Option<&str>,
    ) -> kube::Api<K> {
        kube::Api::all(client)
    }
}

/// `Api<K>` for a namespace (`None` = all namespaces; ignored for cluster-scoped kinds).
pub fn api_for<K: RowSource>(client: kube::Client, namespace: Option<&str>) -> kube::Api<K> {
    <K::Scope as ApiScope>::api::<K>(client, namespace)
}

/// A Kubernetes type that can be shown as a table row.
pub trait RowSource:
    kube::Resource<DynamicType = (), Scope: ApiScope>
    + Clone
    + DeserializeOwned
    + Serialize
    + Debug
    + Send
    + Sync
    + 'static
{
    /// Cells for the kind-specific columns, in `ResourceKind::specific_columns` order.
    fn specific_cells(&self) -> Vec<Cell>;
}

/// Runs `$body` with `$T` bound to the k8s-openapi type of `$kind`.
#[macro_export]
macro_rules! with_kind {
    ($kind:expr, $T:ident => $body:expr) => {{
        use $crate::resources::ResourceKind as K;
        match $kind {
            K::Pods => {
                type $T = k8s_openapi::api::core::v1::Pod;
                $body
            }
            K::Deployments => {
                type $T = k8s_openapi::api::apps::v1::Deployment;
                $body
            }
            K::ReplicaSets => {
                type $T = k8s_openapi::api::apps::v1::ReplicaSet;
                $body
            }
            K::StatefulSets => {
                type $T = k8s_openapi::api::apps::v1::StatefulSet;
                $body
            }
            K::DaemonSets => {
                type $T = k8s_openapi::api::apps::v1::DaemonSet;
                $body
            }
            K::Services => {
                type $T = k8s_openapi::api::core::v1::Service;
                $body
            }
            K::Ingresses => {
                type $T = k8s_openapi::api::networking::v1::Ingress;
                $body
            }
            K::ConfigMaps => {
                type $T = k8s_openapi::api::core::v1::ConfigMap;
                $body
            }
            K::Secrets => {
                type $T = k8s_openapi::api::core::v1::Secret;
                $body
            }
            K::PersistentVolumeClaims => {
                type $T = k8s_openapi::api::core::v1::PersistentVolumeClaim;
                $body
            }
            K::Events => {
                type $T = k8s_openapi::api::core::v1::Event;
                $body
            }
            K::Nodes => {
                type $T = k8s_openapi::api::core::v1::Node;
                $body
            }
            K::Namespaces => {
                type $T = k8s_openapi::api::core::v1::Namespace;
                $body
            }
        }
    }};
}

/// Builds a row; objects without a UID (never the case for server objects) are skipped.
/// `all_namespaces` adds the Namespace cell for namespaced kinds, matching
/// [`crate::resources::columns_for`].
pub fn build_row<K: RowSource>(obj: &K, all_namespaces: bool) -> Option<Row> {
    let include_namespace = all_namespaces && <K::Scope as ApiScope>::NAMESPACED;
    let meta = obj.meta();
    let uid = meta.uid.clone()?;
    let name = obj.name_any();
    let namespace = meta.namespace.clone();

    let mut cells = Vec::with_capacity(8);
    cells.push(Cell::Text(name.clone()));
    if include_namespace {
        cells.push(Cell::text(namespace.clone().unwrap_or_default()));
    }
    cells.extend(obj.specific_cells());

    let mut search_key = String::with_capacity(64);
    for cell in &cells {
        if let Cell::Text(t) | Cell::Status { text: t, .. } = cell {
            search_key.push_str(&t.to_lowercase());
            search_key.push('\u{1f}');
        }
    }
    if !include_namespace && let Some(ns) = &namespace {
        search_key.push_str(&ns.to_lowercase());
    }

    Some(Row {
        uid,
        name,
        namespace,
        cells,
        search_key,
    })
}

pub fn seconds(t: &Time) -> i64 {
    t.0.as_second()
}

fn created(meta: &k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta) -> Cell {
    Cell::time(meta.creation_timestamp.as_ref().map(seconds))
}

fn num(n: Option<i32>) -> Cell {
    Cell::Number(n.unwrap_or(0) as i64)
}

impl RowSource for Pod {
    fn specific_cells(&self) -> Vec<Cell> {
        let summary = pod_summary(self);
        vec![
            Cell::Text(format!("{}/{}", summary.ready, summary.total)),
            Cell::status(summary.reason.clone(), summary.tone),
            Cell::Number(summary.restarts),
            Cell::text(
                self.spec
                    .as_ref()
                    .and_then(|s| s.node_name.clone())
                    .unwrap_or_default(),
            ),
            Cell::text(
                self.status
                    .as_ref()
                    .and_then(|s| s.pod_ip.clone())
                    .unwrap_or_default(),
            ),
            created(&self.metadata),
        ]
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PodSummary {
    pub ready: usize,
    pub total: usize,
    pub restarts: i64,
    pub reason: String,
    pub tone: Tone,
}

/// Pod status the way `kubectl get pods` prints it.
pub fn pod_summary(pod: &Pod) -> PodSummary {
    let status = pod.status.as_ref();
    let total = pod.spec.as_ref().map(|s| s.containers.len()).unwrap_or(0);
    let mut reason = status
        .and_then(|s| s.reason.clone())
        .or_else(|| status.and_then(|s| s.phase.clone()))
        .unwrap_or_else(|| "Unknown".into());

    let init_statuses: &[ContainerStatus] = status
        .and_then(|s| s.init_container_statuses.as_deref())
        .unwrap_or(&[]);
    let statuses: &[ContainerStatus] = status
        .and_then(|s| s.container_statuses.as_deref())
        .unwrap_or(&[]);
    let init_total = pod
        .spec
        .as_ref()
        .and_then(|s| s.init_containers.as_ref())
        .map(Vec::len)
        .unwrap_or(0);

    let mut restarts: i64 = init_statuses
        .iter()
        .chain(statuses)
        .map(|c| c.restart_count as i64)
        .sum();
    restarts = restarts.max(0);

    let mut initializing = false;
    for (i, c) in init_statuses.iter().enumerate() {
        let state = c.state.as_ref();
        if let Some(t) = state.and_then(|s| s.terminated.as_ref()) {
            if t.exit_code == 0 {
                continue;
            }
            reason = match &t.reason {
                Some(r) if !r.is_empty() => format!("Init:{r}"),
                _ if t.signal.unwrap_or(0) != 0 => format!("Init:Signal:{}", t.signal.unwrap_or(0)),
                _ => format!("Init:ExitCode:{}", t.exit_code),
            };
        } else if let Some(r) = state
            .and_then(|s| s.waiting.as_ref())
            .and_then(|w| w.reason.clone())
            .filter(|r| !r.is_empty() && r != "PodInitializing")
        {
            reason = format!("Init:{r}");
        } else {
            reason = format!("Init:{i}/{init_total}");
        }
        initializing = true;
        break;
    }

    let mut ready = 0;
    if !initializing {
        let mut has_running = false;
        for c in statuses.iter().rev() {
            let state = c.state.as_ref();
            if let Some(r) = state
                .and_then(|s| s.waiting.as_ref())
                .and_then(|w| w.reason.clone())
                .filter(|r| !r.is_empty())
            {
                reason = r;
            } else if let Some(t) = state.and_then(|s| s.terminated.as_ref()) {
                reason = match &t.reason {
                    Some(r) if !r.is_empty() => r.clone(),
                    _ if t.signal.unwrap_or(0) != 0 => format!("Signal:{}", t.signal.unwrap_or(0)),
                    _ => format!("ExitCode:{}", t.exit_code),
                };
            } else if c.ready && state.and_then(|s| s.running.as_ref()).is_some() {
                has_running = true;
                ready += 1;
            }
        }
        if reason == "Completed" && has_running {
            reason = "Running".into();
        }
    }

    if pod.metadata.deletion_timestamp.is_some() {
        reason = if status.and_then(|s| s.reason.as_deref()) == Some("NodeLost") {
            "Unknown".into()
        } else {
            "Terminating".into()
        };
    }

    let tone = pod_tone(&reason, ready, total);
    PodSummary {
        ready,
        total,
        restarts,
        reason,
        tone,
    }
}

fn pod_tone(reason: &str, ready: usize, total: usize) -> Tone {
    const ERRORS: [&str; 8] = [
        "Err",
        "Error",
        "BackOff",
        "OOMKilled",
        "Failed",
        "Unknown",
        "Evicted",
        "Invalid",
    ];
    match reason {
        "Running" if ready == total => Tone::Ok,
        "Running" => Tone::Warn,
        "Succeeded" | "Completed" => Tone::Muted,
        r if ERRORS.iter().any(|e| r.contains(e))
            || r.contains("Signal:")
            || r.contains("ExitCode:") =>
        {
            Tone::Error
        }
        _ => Tone::Warn,
    }
}

impl RowSource for Deployment {
    fn specific_cells(&self) -> Vec<Cell> {
        let desired = self.spec.as_ref().and_then(|s| s.replicas).unwrap_or(1);
        let st = self.status.as_ref();
        vec![
            Cell::Text(format!(
                "{}/{}",
                st.and_then(|s| s.ready_replicas).unwrap_or(0),
                desired
            )),
            num(st.and_then(|s| s.updated_replicas)),
            num(st.and_then(|s| s.available_replicas)),
            created(&self.metadata),
        ]
    }
}

impl RowSource for ReplicaSet {
    fn specific_cells(&self) -> Vec<Cell> {
        let st = self.status.as_ref();
        vec![
            num(self.spec.as_ref().and_then(|s| s.replicas).or(Some(1))),
            num(st.map(|s| s.replicas)),
            num(st.and_then(|s| s.ready_replicas)),
            created(&self.metadata),
        ]
    }
}

impl RowSource for StatefulSet {
    fn specific_cells(&self) -> Vec<Cell> {
        let desired = self.spec.as_ref().and_then(|s| s.replicas).unwrap_or(1);
        let ready = self
            .status
            .as_ref()
            .and_then(|s| s.ready_replicas)
            .unwrap_or(0);
        vec![
            Cell::Text(format!("{ready}/{desired}")),
            created(&self.metadata),
        ]
    }
}

impl RowSource for DaemonSet {
    fn specific_cells(&self) -> Vec<Cell> {
        let st = self.status.as_ref();
        vec![
            num(st.map(|s| s.desired_number_scheduled)),
            num(st.map(|s| s.current_number_scheduled)),
            num(st.map(|s| s.number_ready)),
            num(st.and_then(|s| s.updated_number_scheduled)),
            num(st.and_then(|s| s.number_available)),
            created(&self.metadata),
        ]
    }
}

impl RowSource for Service {
    fn specific_cells(&self) -> Vec<Cell> {
        let spec = self.spec.as_ref();
        let svc_type = spec
            .and_then(|s| s.type_.clone())
            .unwrap_or_else(|| "ClusterIP".into());
        vec![
            Cell::text(svc_type.clone()),
            Cell::text(spec.and_then(|s| s.cluster_ip.clone()).unwrap_or_default()),
            Cell::text(service_external_ip(self, &svc_type)),
            Cell::text(service_ports(self)),
            created(&self.metadata),
        ]
    }
}

fn service_external_ip(svc: &Service, svc_type: &str) -> String {
    let lb: Vec<String> = svc
        .status
        .as_ref()
        .and_then(|s| s.load_balancer.as_ref())
        .and_then(|lb| lb.ingress.as_ref())
        .map(|ing| {
            ing.iter()
                .filter_map(|i| i.ip.clone().or_else(|| i.hostname.clone()))
                .collect()
        })
        .unwrap_or_default();
    let external: Vec<String> = svc
        .spec
        .as_ref()
        .and_then(|s| s.external_ips.clone())
        .unwrap_or_default();
    match svc_type {
        "ExternalName" => svc
            .spec
            .as_ref()
            .and_then(|s| s.external_name.clone())
            .unwrap_or_default(),
        "LoadBalancer" if lb.is_empty() && external.is_empty() => "<pending>".into(),
        _ => lb.into_iter().chain(external).collect::<Vec<_>>().join(","),
    }
}

fn service_ports(svc: &Service) -> String {
    let Some(ports) = svc.spec.as_ref().and_then(|s| s.ports.as_ref()) else {
        return String::new();
    };
    ports
        .iter()
        .map(|p| {
            let proto = p.protocol.as_deref().unwrap_or("TCP");
            match p.node_port {
                Some(np) => format!("{}:{np}/{proto}", p.port),
                None => format!("{}/{proto}", p.port),
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

impl RowSource for Ingress {
    fn specific_cells(&self) -> Vec<Cell> {
        let spec = self.spec.as_ref();
        let hosts: Vec<String> = spec
            .and_then(|s| s.rules.as_ref())
            .map(|rules| {
                rules
                    .iter()
                    .map(|r| r.host.clone().unwrap_or_else(|| "*".into()))
                    .collect()
            })
            .unwrap_or_default();
        let address: Vec<String> = self
            .status
            .as_ref()
            .and_then(|s| s.load_balancer.as_ref())
            .and_then(|lb| lb.ingress.as_ref())
            .map(|ing| {
                ing.iter()
                    .filter_map(|i| i.ip.clone().or_else(|| i.hostname.clone()))
                    .collect()
            })
            .unwrap_or_default();
        vec![
            Cell::text(
                spec.and_then(|s| s.ingress_class_name.clone())
                    .unwrap_or_default(),
            ),
            Cell::text(if hosts.is_empty() {
                "*".to_string()
            } else {
                hosts.join(",")
            }),
            Cell::text(address.join(",")),
            created(&self.metadata),
        ]
    }
}

impl RowSource for ConfigMap {
    fn specific_cells(&self) -> Vec<Cell> {
        let n = self.data.as_ref().map(BTreeMap::len).unwrap_or(0)
            + self.binary_data.as_ref().map(BTreeMap::len).unwrap_or(0);
        vec![Cell::Number(n as i64), created(&self.metadata)]
    }
}

impl RowSource for Secret {
    fn specific_cells(&self) -> Vec<Cell> {
        let n = self.data.as_ref().map(BTreeMap::len).unwrap_or(0)
            + self.string_data.as_ref().map(BTreeMap::len).unwrap_or(0);
        vec![
            Cell::text(self.type_.clone().unwrap_or_else(|| "Opaque".into())),
            Cell::Number(n as i64),
            created(&self.metadata),
        ]
    }
}

impl RowSource for PersistentVolumeClaim {
    fn specific_cells(&self) -> Vec<Cell> {
        let st = self.status.as_ref();
        let phase = st
            .and_then(|s| s.phase.clone())
            .unwrap_or_else(|| "Unknown".into());
        let tone = match phase.as_str() {
            "Bound" => Tone::Ok,
            "Lost" => Tone::Error,
            _ => Tone::Warn,
        };
        let capacity = st
            .and_then(|s| s.capacity.as_ref())
            .and_then(|c| c.get("storage"))
            .map(|q| q.0.clone())
            .unwrap_or_default();
        let modes = st
            .and_then(|s| s.access_modes.as_ref())
            .map(|m| {
                m.iter()
                    .map(|m| access_mode_short(m))
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_default();
        let spec = self.spec.as_ref();
        vec![
            Cell::status(phase, tone),
            Cell::text(spec.and_then(|s| s.volume_name.clone()).unwrap_or_default()),
            Cell::text(capacity),
            Cell::text(modes),
            Cell::text(
                spec.and_then(|s| s.storage_class_name.clone())
                    .unwrap_or_default(),
            ),
            created(&self.metadata),
        ]
    }
}

pub fn access_mode_short(mode: &str) -> &str {
    match mode {
        "ReadWriteOnce" => "RWO",
        "ReadOnlyMany" => "ROX",
        "ReadWriteMany" => "RWX",
        "ReadWriteOncePod" => "RWOP",
        other => other,
    }
}

impl RowSource for Event {
    fn specific_cells(&self) -> Vec<Cell> {
        let last_seen = self
            .series
            .as_ref()
            .and_then(|s| s.last_observed_time.as_ref())
            .map(|t| t.0.as_second())
            .or_else(|| self.last_timestamp.as_ref().map(seconds))
            .or_else(|| self.event_time.as_ref().map(|t| t.0.as_second()))
            .or_else(|| self.metadata.creation_timestamp.as_ref().map(seconds));
        let event_type = self.type_.clone().unwrap_or_else(|| "Normal".into());
        let tone = if event_type == "Warning" {
            Tone::Warn
        } else {
            Tone::Muted
        };
        let obj = &self.involved_object;
        let object = format!(
            "{}/{}",
            obj.kind.as_deref().unwrap_or("").to_lowercase(),
            obj.name.as_deref().unwrap_or("")
        );
        let count = self
            .series
            .as_ref()
            .and_then(|s| s.count)
            .or(self.count)
            .unwrap_or(1);
        let message = self
            .message
            .as_deref()
            .unwrap_or("")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        vec![
            Cell::time(last_seen),
            Cell::status(event_type, tone),
            Cell::text(self.reason.clone().unwrap_or_default()),
            Cell::text(object),
            Cell::Number(count as i64),
            Cell::text(message),
        ]
    }
}

impl RowSource for Node {
    fn specific_cells(&self) -> Vec<Cell> {
        let st = self.status.as_ref();
        let ready = st
            .and_then(|s| s.conditions.as_ref())
            .and_then(|c| c.iter().find(|c| c.type_ == "Ready"))
            .map(|c| c.status.as_str());
        let (mut text, mut tone) = match ready {
            Some("True") => ("Ready".to_string(), Tone::Ok),
            Some("False") => ("NotReady".to_string(), Tone::Error),
            _ => ("Unknown".to_string(), Tone::Error),
        };
        if self
            .spec
            .as_ref()
            .and_then(|s| s.unschedulable)
            .unwrap_or(false)
        {
            text.push_str(",SchedulingDisabled");
            if tone == Tone::Ok {
                tone = Tone::Warn;
            }
        }
        let internal_ip = st
            .and_then(|s| s.addresses.as_ref())
            .and_then(|a| a.iter().find(|a| a.type_ == "InternalIP"))
            .map(|a| a.address.clone())
            .unwrap_or_default();
        vec![
            Cell::status(text, tone),
            Cell::text(node_roles(self)),
            Cell::text(
                st.and_then(|s| s.node_info.as_ref())
                    .map(|i| i.kubelet_version.clone())
                    .unwrap_or_default(),
            ),
            Cell::text(internal_ip),
            created(&self.metadata),
        ]
    }
}

pub fn node_roles(node: &Node) -> String {
    let mut roles: Vec<&str> = node
        .labels()
        .iter()
        .filter_map(|(k, v)| {
            if let Some(role) = k.strip_prefix("node-role.kubernetes.io/") {
                Some(role)
            } else if k == "kubernetes.io/role" {
                Some(v.as_str())
            } else {
                None
            }
        })
        .filter(|r| !r.is_empty())
        .collect();
    roles.sort_unstable();
    roles.dedup();
    if roles.is_empty() {
        "<none>".into()
    } else {
        roles.join(",")
    }
}

impl RowSource for Namespace {
    fn specific_cells(&self) -> Vec<Cell> {
        let phase = self
            .status
            .as_ref()
            .and_then(|s| s.phase.clone())
            .unwrap_or_else(|| "Active".into());
        let tone = if phase == "Active" {
            Tone::Ok
        } else {
            Tone::Warn
        };
        vec![Cell::status(phase, tone), created(&self.metadata)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::{ResourceKind, columns_for};
    use serde_json::json;

    fn pod(value: serde_json::Value) -> Pod {
        serde_json::from_value(value).unwrap()
    }

    fn base_pod(statuses: serde_json::Value) -> Pod {
        pod(json!({
            "metadata": { "name": "web-1", "namespace": "shop", "uid": "u1", "creationTimestamp": "2026-01-01T00:00:00Z" },
            "spec": { "containers": [{ "name": "app" }, { "name": "sidecar" }], "nodeName": "node-a" },
            "status": { "phase": "Running", "podIP": "10.0.0.5", "containerStatuses": statuses }
        }))
    }

    #[test]
    fn running_pod_all_ready() {
        let p = base_pod(json!([
            { "name": "app", "ready": true, "restartCount": 2, "image": "i", "imageID": "", "state": { "running": {} } },
            { "name": "sidecar", "ready": true, "restartCount": 1, "image": "i", "imageID": "", "state": { "running": {} } }
        ]));
        let s = pod_summary(&p);
        assert_eq!((s.ready, s.total, s.restarts), (2, 2, 3));
        assert_eq!(s.reason, "Running");
        assert_eq!(s.tone, Tone::Ok);
    }

    #[test]
    fn crashloop_wins_over_phase() {
        let p = base_pod(json!([
            { "name": "app", "ready": false, "restartCount": 7, "image": "i", "imageID": "", "state": { "waiting": { "reason": "CrashLoopBackOff" } } },
            { "name": "sidecar", "ready": true, "restartCount": 0, "image": "i", "imageID": "", "state": { "running": {} } }
        ]));
        let s = pod_summary(&p);
        assert_eq!(s.reason, "CrashLoopBackOff");
        assert_eq!(s.tone, Tone::Error);
        assert_eq!(s.ready, 1);
    }

    #[test]
    fn init_progress_and_terminating() {
        let mut p = pod(json!({
            "metadata": { "name": "job", "uid": "u2" },
            "spec": { "containers": [{ "name": "app" }], "initContainers": [{ "name": "a" }, { "name": "b" }] },
            "status": { "phase": "Pending", "initContainerStatuses": [
                { "name": "a", "ready": true, "restartCount": 0, "image": "i", "imageID": "", "state": { "terminated": { "exitCode": 0 } } },
                { "name": "b", "ready": false, "restartCount": 0, "image": "i", "imageID": "", "state": { "running": {} } }
            ] }
        }));
        assert_eq!(pod_summary(&p).reason, "Init:1/2");
        p.metadata.deletion_timestamp = p
            .metadata
            .creation_timestamp
            .clone()
            .or(Some(Time("2026-01-01T00:00:00Z".parse().unwrap())));
        assert_eq!(pod_summary(&p).reason, "Terminating");
    }

    #[test]
    fn row_has_one_cell_per_column() {
        let p = base_pod(json!([]));
        for all in [false, true] {
            let row = build_row(&p, all).unwrap();
            assert_eq!(row.cells.len(), columns_for(ResourceKind::Pods, all).len());
            assert!(row.search_key.contains("web-1"));
            assert!(
                row.search_key.contains("shop"),
                "namespace must be searchable in both modes"
            );
            assert!(row.search_key.contains("node-a"));
        }
    }

    #[test]
    fn every_kind_matches_its_column_count() {
        // Minimal objects for each kind must produce exactly one cell per column.
        let meta = json!({ "name": "x", "namespace": "ns", "uid": "u" });
        let samples: Vec<(ResourceKind, Option<Row>)> = vec![
            (
                ResourceKind::Pods,
                build_row(
                    &serde_json::from_value::<Pod>(json!({ "metadata": meta })).unwrap(),
                    true,
                ),
            ),
            (
                ResourceKind::Deployments,
                build_row(
                    &serde_json::from_value::<Deployment>(json!({ "metadata": meta })).unwrap(),
                    true,
                ),
            ),
            (
                ResourceKind::ReplicaSets,
                build_row(
                    &serde_json::from_value::<ReplicaSet>(json!({ "metadata": meta })).unwrap(),
                    true,
                ),
            ),
            (
                ResourceKind::StatefulSets,
                build_row(
                    &serde_json::from_value::<StatefulSet>(json!({ "metadata": meta })).unwrap(),
                    true,
                ),
            ),
            (
                ResourceKind::DaemonSets,
                build_row(
                    &serde_json::from_value::<DaemonSet>(json!({ "metadata": meta })).unwrap(),
                    true,
                ),
            ),
            (
                ResourceKind::Services,
                build_row(
                    &serde_json::from_value::<Service>(json!({ "metadata": meta })).unwrap(),
                    true,
                ),
            ),
            (
                ResourceKind::Ingresses,
                build_row(
                    &serde_json::from_value::<Ingress>(json!({ "metadata": meta })).unwrap(),
                    true,
                ),
            ),
            (
                ResourceKind::ConfigMaps,
                build_row(
                    &serde_json::from_value::<ConfigMap>(json!({ "metadata": meta })).unwrap(),
                    true,
                ),
            ),
            (
                ResourceKind::Secrets,
                build_row(
                    &serde_json::from_value::<Secret>(json!({ "metadata": meta })).unwrap(),
                    true,
                ),
            ),
            (
                ResourceKind::PersistentVolumeClaims,
                build_row(
                    &serde_json::from_value::<PersistentVolumeClaim>(json!({ "metadata": meta }))
                        .unwrap(),
                    true,
                ),
            ),
            (
                ResourceKind::Events,
                build_row(
                    &serde_json::from_value::<Event>(
                        json!({ "metadata": meta, "involvedObject": {} }),
                    )
                    .unwrap(),
                    true,
                ),
            ),
            (
                ResourceKind::Nodes,
                build_row(
                    &serde_json::from_value::<Node>(
                        json!({ "metadata": { "name": "n", "uid": "u" } }),
                    )
                    .unwrap(),
                    true,
                ),
            ),
            (
                ResourceKind::Namespaces,
                build_row(
                    &serde_json::from_value::<Namespace>(
                        json!({ "metadata": { "name": "n", "uid": "u" } }),
                    )
                    .unwrap(),
                    true,
                ),
            ),
        ];
        assert_eq!(samples.len(), ResourceKind::ALL.len());
        for (kind, row) in samples {
            let row = row.unwrap();
            assert_eq!(row.cells.len(), columns_for(kind, true).len(), "{kind:?}");
        }
    }

    #[test]
    fn kind_scope_matches_type_scope() {
        for kind in ResourceKind::ALL {
            let namespaced = crate::with_kind!(kind, T => <<T as kube::Resource>::Scope as ApiScope>::NAMESPACED);
            assert_eq!(namespaced, kind.namespaced(), "{kind:?}");
        }
    }

    #[test]
    fn service_columns() {
        let svc: Service = serde_json::from_value(json!({
            "metadata": { "name": "api", "uid": "u" },
            "spec": { "type": "LoadBalancer", "clusterIP": "10.96.0.10", "ports": [
                { "port": 80, "nodePort": 30080, "protocol": "TCP" }, { "port": 53, "protocol": "UDP" }
            ] }
        }))
        .unwrap();
        let cells = svc.specific_cells();
        assert_eq!(cells[2], Cell::text("<pending>"));
        assert_eq!(cells[3], Cell::text("80:30080/TCP,53/UDP"));
    }

    #[test]
    fn node_status_and_roles() {
        let node: Node = serde_json::from_value(json!({
            "metadata": { "name": "cp", "uid": "u", "labels": { "node-role.kubernetes.io/control-plane": "" } },
            "spec": { "unschedulable": true },
            "status": { "conditions": [{ "type": "Ready", "status": "True" }],
                        "addresses": [{ "type": "InternalIP", "address": "172.18.0.2" }] }
        }))
        .unwrap();
        let cells = node.specific_cells();
        assert_eq!(
            cells[0],
            Cell::status("Ready,SchedulingDisabled", Tone::Warn)
        );
        assert_eq!(cells[1], Cell::text("control-plane"));
        assert_eq!(cells[3], Cell::text("172.18.0.2"));
    }
}
