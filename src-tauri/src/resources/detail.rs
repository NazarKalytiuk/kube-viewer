//! Structured detail (Status / Labels / YAML tabs) for a single object.

use std::collections::BTreeMap;

use k8s_openapi::api::apps::v1::{DaemonSet, Deployment, ReplicaSet, StatefulSet};
use k8s_openapi::api::core::v1::{
    ConfigMap, Event, Namespace, Node, PersistentVolumeClaim, Pod, PodSpec, Secret, Service,
};
use k8s_openapi::api::networking::v1::Ingress;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;
use kube::ResourceExt;
use serde::Serialize;

use super::rows::{RowSource, access_mode_short, node_roles, pod_summary, seconds};
use super::yaml::{display_value, to_yaml};
use crate::error::AppResult;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Field {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Section {
    pub title: String,
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Condition {
    #[serde(rename = "type")]
    pub type_: String,
    pub status: String,
    pub reason: Option<String>,
    pub message: Option<String>,
    pub last_transition: Option<i64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ContainerRef {
    pub name: String,
    pub init: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Detail {
    pub name: String,
    pub namespace: Option<String>,
    pub uid: Option<String>,
    pub resource_version: Option<String>,
    pub created: Option<i64>,
    pub labels: BTreeMap<String, String>,
    pub annotations: BTreeMap<String, String>,
    pub sections: Vec<Section>,
    pub conditions: Vec<Condition>,
    /// Pods only: containers whose logs can be streamed.
    pub containers: Vec<ContainerRef>,
    pub yaml: String,
}

/// Per-kind Status tab content.
pub trait DetailSource: RowSource {
    const IS_SECRET: bool = false;
    fn sections(&self) -> Vec<Section>;
    fn conditions(&self) -> Vec<Condition> {
        Vec::new()
    }
    fn containers(&self) -> Vec<ContainerRef> {
        Vec::new()
    }
}

pub fn build_detail<K: DetailSource>(obj: &K) -> AppResult<Detail> {
    let display = display_value(obj, K::IS_SECRET)?;
    let meta = obj.meta();
    let mut annotations = meta.annotations.clone().unwrap_or_default();
    if K::IS_SECRET
        && let Some(v) = annotations.get_mut("kubectl.kubernetes.io/last-applied-configuration")
    {
        *v = "<hidden>".into();
    }
    Ok(Detail {
        name: obj.name_any(),
        namespace: meta.namespace.clone(),
        uid: meta.uid.clone(),
        resource_version: meta.resource_version.clone(),
        created: meta.creation_timestamp.as_ref().map(seconds),
        labels: meta.labels.clone().unwrap_or_default(),
        annotations,
        sections: obj.sections(),
        conditions: obj.conditions(),
        containers: obj.containers(),
        yaml: to_yaml(&display)?,
    })
}

fn field(label: &str, value: impl Into<String>) -> Option<Field> {
    let value = value.into();
    (!value.is_empty()).then(|| Field {
        label: label.to_string(),
        value,
    })
}

fn section(title: &str, fields: impl IntoIterator<Item = Option<Field>>) -> Section {
    Section {
        title: title.to_string(),
        fields: fields.into_iter().flatten().collect(),
    }
}

fn opt_num<T: ToString>(v: Option<T>) -> String {
    v.map(|n| n.to_string()).unwrap_or_else(|| "0".into())
}

fn selector_text(sel: Option<&LabelSelector>) -> String {
    let Some(sel) = sel else { return String::new() };
    let mut parts: Vec<String> = sel
        .match_labels
        .iter()
        .flatten()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    for expr in sel.match_expressions.iter().flatten() {
        let values = expr.values.clone().unwrap_or_default().join(",");
        parts.push(format!("{} {} ({values})", expr.key, expr.operator));
    }
    parts.join(", ")
}

fn map_text(map: Option<&BTreeMap<String, String>>) -> String {
    map.map(|m| {
        m.iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(", ")
    })
    .unwrap_or_default()
}

macro_rules! conditions_from {
    ($conds:expr) => {
        $conds
            .into_iter()
            .flatten()
            .map(|c| Condition {
                type_: c.type_.clone(),
                status: c.status.clone(),
                reason: c.reason.clone(),
                message: c.message.clone(),
                last_transition: c.last_transition_time.as_ref().map(seconds),
            })
            .collect()
    };
}

fn pod_template_section(spec: Option<&PodSpec>) -> Section {
    let images = spec
        .map(|s| {
            s.containers
                .iter()
                .map(|c| format!("{}: {}", c.name, c.image.clone().unwrap_or_default()))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    section(
        "Pod template",
        [
            field("Images", images),
            field(
                "Service account",
                spec.and_then(|s| s.service_account_name.clone())
                    .unwrap_or_default(),
            ),
            field(
                "Node selector",
                map_text(spec.and_then(|s| s.node_selector.as_ref())),
            ),
        ],
    )
}

impl DetailSource for Pod {
    fn sections(&self) -> Vec<Section> {
        let s = pod_summary(self);
        let spec = self.spec.as_ref();
        let st = self.status.as_ref();
        let owners = self
            .owner_references()
            .iter()
            .map(|o| format!("{}/{}", o.kind, o.name))
            .collect::<Vec<_>>()
            .join(", ");
        let mut sections = vec![section(
            "Pod",
            [
                field("Status", s.reason),
                field("Ready", format!("{}/{}", s.ready, s.total)),
                field("Restarts", s.restarts.to_string()),
                field(
                    "Node",
                    spec.and_then(|s| s.node_name.clone()).unwrap_or_default(),
                ),
                field(
                    "Pod IP",
                    st.and_then(|s| s.pod_ip.clone()).unwrap_or_default(),
                ),
                field(
                    "Host IP",
                    st.and_then(|s| s.host_ip.clone()).unwrap_or_default(),
                ),
                field(
                    "QoS class",
                    st.and_then(|s| s.qos_class.clone()).unwrap_or_default(),
                ),
                field(
                    "Service account",
                    spec.and_then(|s| s.service_account_name.clone())
                        .unwrap_or_default(),
                ),
                field("Controlled by", owners),
            ],
        )];
        let statuses = st.and_then(|s| s.container_statuses.as_ref());
        for c in spec.map(|s| s.containers.as_slice()).unwrap_or(&[]) {
            let cs = statuses.and_then(|all| all.iter().find(|s| s.name == c.name));
            let state = cs.and_then(|s| s.state.as_ref()).map(|state| {
                if let Some(w) = &state.waiting {
                    format!("Waiting: {}", w.reason.clone().unwrap_or_default())
                } else if let Some(t) = &state.terminated {
                    format!(
                        "Terminated: {} (exit {})",
                        t.reason.clone().unwrap_or_default(),
                        t.exit_code
                    )
                } else {
                    "Running".to_string()
                }
            });
            let ports = c
                .ports
                .iter()
                .flatten()
                .map(|p| {
                    format!(
                        "{}/{}",
                        p.container_port,
                        p.protocol.as_deref().unwrap_or("TCP")
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            sections.push(section(
                &format!("Container {}", c.name),
                [
                    field("Image", c.image.clone().unwrap_or_default()),
                    field("State", state.unwrap_or_default()),
                    field("Ready", cs.map(|s| s.ready.to_string()).unwrap_or_default()),
                    field(
                        "Restarts",
                        cs.map(|s| s.restart_count.to_string()).unwrap_or_default(),
                    ),
                    field("Ports", ports),
                ],
            ));
        }
        sections
    }

    fn conditions(&self) -> Vec<Condition> {
        conditions_from!(self.status.as_ref().and_then(|s| s.conditions.as_ref()))
    }

    fn containers(&self) -> Vec<ContainerRef> {
        let Some(spec) = &self.spec else {
            return Vec::new();
        };
        let init = spec.init_containers.iter().flatten().map(|c| ContainerRef {
            name: c.name.clone(),
            init: true,
        });
        let main = spec.containers.iter().map(|c| ContainerRef {
            name: c.name.clone(),
            init: false,
        });
        main.chain(init).collect()
    }
}

impl DetailSource for Deployment {
    fn sections(&self) -> Vec<Section> {
        let spec = self.spec.as_ref();
        let st = self.status.as_ref();
        vec![
            section(
                "Deployment",
                [
                    field(
                        "Replicas",
                        format!(
                            "{} desired, {} updated, {} ready, {} available",
                            spec.and_then(|s| s.replicas).unwrap_or(1),
                            opt_num(st.and_then(|s| s.updated_replicas)),
                            opt_num(st.and_then(|s| s.ready_replicas)),
                            opt_num(st.and_then(|s| s.available_replicas)),
                        ),
                    ),
                    field(
                        "Strategy",
                        spec.and_then(|s| s.strategy.as_ref())
                            .and_then(|s| s.type_.clone())
                            .unwrap_or_default(),
                    ),
                    field("Selector", selector_text(spec.map(|s| &s.selector))),
                    field(
                        "Paused",
                        if spec.and_then(|s| s.paused).unwrap_or(false) {
                            "true"
                        } else {
                            ""
                        },
                    ),
                ],
            ),
            pod_template_section(spec.and_then(|s| s.template.spec.as_ref())),
        ]
    }

    fn conditions(&self) -> Vec<Condition> {
        conditions_from!(self.status.as_ref().and_then(|s| s.conditions.as_ref()))
    }
}

impl DetailSource for ReplicaSet {
    fn sections(&self) -> Vec<Section> {
        let spec = self.spec.as_ref();
        let st = self.status.as_ref();
        vec![
            section(
                "ReplicaSet",
                [
                    field(
                        "Replicas",
                        format!(
                            "{} desired, {} current, {} ready",
                            spec.and_then(|s| s.replicas).unwrap_or(1),
                            st.map(|s| s.replicas).unwrap_or(0),
                            opt_num(st.and_then(|s| s.ready_replicas)),
                        ),
                    ),
                    field("Selector", selector_text(spec.map(|s| &s.selector))),
                    field(
                        "Controlled by",
                        self.owner_references()
                            .iter()
                            .map(|o| format!("{}/{}", o.kind, o.name))
                            .collect::<Vec<_>>()
                            .join(", "),
                    ),
                ],
            ),
            pod_template_section(
                spec.and_then(|s| s.template.as_ref())
                    .and_then(|t| t.spec.as_ref()),
            ),
        ]
    }

    fn conditions(&self) -> Vec<Condition> {
        conditions_from!(self.status.as_ref().and_then(|s| s.conditions.as_ref()))
    }
}

impl DetailSource for StatefulSet {
    fn sections(&self) -> Vec<Section> {
        let spec = self.spec.as_ref();
        let st = self.status.as_ref();
        vec![
            section(
                "StatefulSet",
                [
                    field(
                        "Replicas",
                        format!(
                            "{} desired, {} ready, {} current",
                            spec.and_then(|s| s.replicas).unwrap_or(1),
                            opt_num(st.and_then(|s| s.ready_replicas)),
                            opt_num(st.and_then(|s| s.current_replicas)),
                        ),
                    ),
                    field(
                        "Service",
                        spec.and_then(|s| s.service_name.clone())
                            .unwrap_or_default(),
                    ),
                    field(
                        "Update strategy",
                        spec.and_then(|s| s.update_strategy.as_ref())
                            .and_then(|s| s.type_.clone())
                            .unwrap_or_default(),
                    ),
                    field("Selector", selector_text(spec.map(|s| &s.selector))),
                ],
            ),
            pod_template_section(spec.and_then(|s| s.template.spec.as_ref())),
        ]
    }

    fn conditions(&self) -> Vec<Condition> {
        conditions_from!(self.status.as_ref().and_then(|s| s.conditions.as_ref()))
    }
}

impl DetailSource for DaemonSet {
    fn sections(&self) -> Vec<Section> {
        let spec = self.spec.as_ref();
        let st = self.status.as_ref();
        vec![
            section(
                "DaemonSet",
                [
                    field(
                        "Scheduled",
                        format!(
                            "{} desired, {} current, {} ready, {} misscheduled",
                            st.map(|s| s.desired_number_scheduled).unwrap_or(0),
                            st.map(|s| s.current_number_scheduled).unwrap_or(0),
                            st.map(|s| s.number_ready).unwrap_or(0),
                            st.map(|s| s.number_misscheduled).unwrap_or(0),
                        ),
                    ),
                    field(
                        "Update strategy",
                        spec.and_then(|s| s.update_strategy.as_ref())
                            .and_then(|s| s.type_.clone())
                            .unwrap_or_default(),
                    ),
                    field("Selector", selector_text(spec.map(|s| &s.selector))),
                ],
            ),
            pod_template_section(spec.and_then(|s| s.template.spec.as_ref())),
        ]
    }

    fn conditions(&self) -> Vec<Condition> {
        conditions_from!(self.status.as_ref().and_then(|s| s.conditions.as_ref()))
    }
}

impl DetailSource for Service {
    fn sections(&self) -> Vec<Section> {
        let spec = self.spec.as_ref();
        let ports = spec
            .and_then(|s| s.ports.as_ref())
            .map(|ports| {
                ports
                    .iter()
                    .map(|p| {
                        let target = p.target_port.as_ref().map(|t| match t {
                            k8s_openapi::apimachinery::pkg::util::intstr::IntOrString::Int(i) => {
                                i.to_string()
                            }
                            k8s_openapi::apimachinery::pkg::util::intstr::IntOrString::String(
                                s,
                            ) => s.clone(),
                        });
                        format!(
                            "{}{} → {}/{}",
                            p.name
                                .as_ref()
                                .map(|n| format!("{n}: "))
                                .unwrap_or_default(),
                            p.port,
                            target.unwrap_or_else(|| p.port.to_string()),
                            p.protocol.as_deref().unwrap_or("TCP"),
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        vec![section(
            "Service",
            [
                field(
                    "Type",
                    spec.and_then(|s| s.type_.clone())
                        .unwrap_or_else(|| "ClusterIP".into()),
                ),
                field(
                    "Cluster IPs",
                    spec.and_then(|s| s.cluster_ips.clone())
                        .unwrap_or_default()
                        .join(", "),
                ),
                field("Ports", ports),
                field("Selector", map_text(spec.and_then(|s| s.selector.as_ref()))),
                field(
                    "Session affinity",
                    spec.and_then(|s| s.session_affinity.clone())
                        .unwrap_or_default(),
                ),
            ],
        )]
    }
}

impl DetailSource for Ingress {
    fn sections(&self) -> Vec<Section> {
        let spec = self.spec.as_ref();
        let rules = spec
            .and_then(|s| s.rules.as_ref())
            .map(|rules| {
                rules
                    .iter()
                    .flat_map(|r| {
                        let host = r.host.clone().unwrap_or_else(|| "*".into());
                        r.http.iter().flat_map(|h| h.paths.iter()).map(move |p| {
                            let backend = p
                                .backend
                                .service
                                .as_ref()
                                .map(|s| {
                                    let port = s.port.as_ref().map(|p| {
                                        p.number
                                            .map(|n| n.to_string())
                                            .or_else(|| p.name.clone())
                                            .unwrap_or_default()
                                    });
                                    format!("{}:{}", s.name, port.unwrap_or_default())
                                })
                                .unwrap_or_default();
                            format!("{host}{} → {backend}", p.path.as_deref().unwrap_or("/"))
                        })
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        let tls = spec
            .and_then(|s| s.tls.as_ref())
            .map(|t| {
                t.iter()
                    .flat_map(|t| t.hosts.clone().unwrap_or_default())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        vec![section(
            "Ingress",
            [
                field(
                    "Class",
                    spec.and_then(|s| s.ingress_class_name.clone())
                        .unwrap_or_default(),
                ),
                field("Rules", rules),
                field("TLS hosts", tls),
            ],
        )]
    }
}

impl DetailSource for ConfigMap {
    fn sections(&self) -> Vec<Section> {
        let data = self
            .data
            .iter()
            .flatten()
            .map(|(k, v)| field(k, format!("{} bytes", v.len())));
        let binary = self
            .binary_data
            .iter()
            .flatten()
            .map(|(k, v)| field(k, format!("{} bytes (binary)", v.0.len())));
        vec![section("Data", data.chain(binary))]
    }
}

impl DetailSource for Secret {
    const IS_SECRET: bool = true;
    fn sections(&self) -> Vec<Section> {
        let data = self
            .data
            .iter()
            .flatten()
            .map(|(k, v)| field(k, format!("{} bytes", v.0.len())));
        vec![
            section(
                "Secret",
                [field(
                    "Type",
                    self.type_.clone().unwrap_or_else(|| "Opaque".into()),
                )],
            ),
            section("Data", data),
        ]
    }
}

impl DetailSource for PersistentVolumeClaim {
    fn sections(&self) -> Vec<Section> {
        let spec = self.spec.as_ref();
        let st = self.status.as_ref();
        let requested = spec
            .and_then(|s| s.resources.as_ref())
            .and_then(|r| r.requests.as_ref())
            .and_then(|r| r.get("storage"))
            .map(|q| q.0.clone())
            .unwrap_or_default();
        vec![section(
            "PersistentVolumeClaim",
            [
                field(
                    "Status",
                    st.and_then(|s| s.phase.clone()).unwrap_or_default(),
                ),
                field(
                    "Volume",
                    spec.and_then(|s| s.volume_name.clone()).unwrap_or_default(),
                ),
                field("Requested", requested),
                field(
                    "Capacity",
                    st.and_then(|s| s.capacity.as_ref())
                        .and_then(|c| c.get("storage"))
                        .map(|q| q.0.clone())
                        .unwrap_or_default(),
                ),
                field(
                    "Access modes",
                    spec.and_then(|s| s.access_modes.as_ref())
                        .map(|m| {
                            m.iter()
                                .map(|m| access_mode_short(m))
                                .collect::<Vec<_>>()
                                .join(", ")
                        })
                        .unwrap_or_default(),
                ),
                field(
                    "Storage class",
                    spec.and_then(|s| s.storage_class_name.clone())
                        .unwrap_or_default(),
                ),
                field(
                    "Volume mode",
                    spec.and_then(|s| s.volume_mode.clone()).unwrap_or_default(),
                ),
            ],
        )]
    }

    fn conditions(&self) -> Vec<Condition> {
        conditions_from!(self.status.as_ref().and_then(|s| s.conditions.as_ref()))
    }
}

impl DetailSource for Event {
    fn sections(&self) -> Vec<Section> {
        let obj = &self.involved_object;
        vec![section(
            "Event",
            [
                field("Type", self.type_.clone().unwrap_or_default()),
                field("Reason", self.reason.clone().unwrap_or_default()),
                field("Message", self.message.clone().unwrap_or_default()),
                field(
                    "Object",
                    format!(
                        "{}/{}",
                        obj.kind.clone().unwrap_or_default(),
                        obj.name.clone().unwrap_or_default()
                    ),
                ),
                field(
                    "Object namespace",
                    obj.namespace.clone().unwrap_or_default(),
                ),
                field(
                    "Count",
                    self.count.map(|c| c.to_string()).unwrap_or_default(),
                ),
                field(
                    "Source",
                    self.source
                        .as_ref()
                        .map(|s| {
                            [s.component.clone(), s.host.clone()]
                                .into_iter()
                                .flatten()
                                .collect::<Vec<_>>()
                                .join(" on ")
                        })
                        .or_else(|| self.reporting_component.clone())
                        .unwrap_or_default(),
                ),
            ],
        )]
    }
}

impl DetailSource for Node {
    fn sections(&self) -> Vec<Section> {
        let st = self.status.as_ref();
        let info = st.and_then(|s| s.node_info.as_ref());
        let quantity = |map: Option<
            &BTreeMap<String, k8s_openapi::apimachinery::pkg::api::resource::Quantity>,
        >,
                        key: &str| {
            map.and_then(|m| m.get(key))
                .map(|q| q.0.clone())
                .unwrap_or_default()
        };
        let capacity = st.and_then(|s| s.capacity.as_ref());
        let allocatable = st.and_then(|s| s.allocatable.as_ref());
        let addresses = st
            .and_then(|s| s.addresses.as_ref())
            .map(|a| {
                a.iter()
                    .map(|a| format!("{}: {}", a.type_, a.address))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        let taints = self
            .spec
            .as_ref()
            .and_then(|s| s.taints.as_ref())
            .map(|t| {
                t.iter()
                    .map(|t| {
                        format!(
                            "{}={}:{}",
                            t.key,
                            t.value.clone().unwrap_or_default(),
                            t.effect
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        vec![
            section(
                "Node",
                [
                    field("Roles", node_roles(self)),
                    field("Addresses", addresses),
                    field("Taints", taints),
                    field(
                        "Pod CIDR",
                        self.spec
                            .as_ref()
                            .and_then(|s| s.pod_cidr.clone())
                            .unwrap_or_default(),
                    ),
                ],
            ),
            section(
                "System",
                [
                    field(
                        "Kubelet",
                        info.map(|i| i.kubelet_version.clone()).unwrap_or_default(),
                    ),
                    field(
                        "OS image",
                        info.map(|i| i.os_image.clone()).unwrap_or_default(),
                    ),
                    field(
                        "Kernel",
                        info.map(|i| i.kernel_version.clone()).unwrap_or_default(),
                    ),
                    field(
                        "Container runtime",
                        info.map(|i| i.container_runtime_version.clone())
                            .unwrap_or_default(),
                    ),
                    field(
                        "Architecture",
                        info.map(|i| format!("{}/{}", i.operating_system, i.architecture))
                            .unwrap_or_default(),
                    ),
                ],
            ),
            section(
                "Capacity (allocatable)",
                ["cpu", "memory", "pods", "ephemeral-storage"].map(|key| {
                    let cap = quantity(capacity, key);
                    let alloc = quantity(allocatable, key);
                    field(
                        key,
                        if cap.is_empty() {
                            String::new()
                        } else {
                            format!("{cap} ({alloc})")
                        },
                    )
                }),
            ),
        ]
    }

    fn conditions(&self) -> Vec<Condition> {
        self.status
            .as_ref()
            .and_then(|s| s.conditions.as_ref())
            .iter()
            .flat_map(|c| c.iter())
            .map(|c| Condition {
                type_: c.type_.clone(),
                status: c.status.clone(),
                reason: c.reason.clone(),
                message: c.message.clone(),
                last_transition: c.last_transition_time.as_ref().map(seconds),
            })
            .collect()
    }
}

impl DetailSource for Namespace {
    fn sections(&self) -> Vec<Section> {
        vec![section(
            "Namespace",
            [field(
                "Status",
                self.status
                    .as_ref()
                    .and_then(|s| s.phase.clone())
                    .unwrap_or_default(),
            )],
        )]
    }

    fn conditions(&self) -> Vec<Condition> {
        conditions_from!(self.status.as_ref().and_then(|s| s.conditions.as_ref()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pod_detail_lists_containers_and_conditions() {
        let pod: Pod = serde_json::from_value(json!({
            "metadata": { "name": "web", "namespace": "shop", "uid": "u", "labels": { "app": "web" } },
            "spec": { "containers": [{ "name": "app", "image": "nginx:1.29" }], "initContainers": [{ "name": "migrate" }] },
            "status": { "phase": "Running", "conditions": [{ "type": "Ready", "status": "True" }] }
        }))
        .unwrap();
        let detail = build_detail(&pod).unwrap();
        assert_eq!(detail.labels.get("app").map(String::as_str), Some("web"));
        assert_eq!(
            detail.containers,
            vec![
                ContainerRef {
                    name: "app".into(),
                    init: false
                },
                ContainerRef {
                    name: "migrate".into(),
                    init: true
                }
            ]
        );
        assert_eq!(detail.conditions[0].type_, "Ready");
        assert!(detail.sections.iter().any(|s| s.title == "Container app"));
        assert!(detail.yaml.contains("nginx:1.29"));
    }

    #[test]
    fn secret_detail_never_contains_values() {
        let secret: Secret = serde_json::from_value(json!({
            "metadata": { "name": "db", "uid": "u", "annotations": {
                "kubectl.kubernetes.io/last-applied-configuration": "{\"data\":{\"password\":\"c2VjcmV0\"}}"
            } },
            "data": { "password": "c2VjcmV0" }
        }))
        .unwrap();
        let detail = build_detail(&secret).unwrap();
        let json = serde_json::to_string(&detail).unwrap();
        assert!(!json.contains("c2VjcmV0"), "secret leaked: {json}");
        assert!(json.contains("6 bytes"));
    }
}
