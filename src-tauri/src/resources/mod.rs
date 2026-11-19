//! Supported resource kinds, their table columns and the row model.
//!
//! Rows are computed in Rust from typed objects; only compact rows (never the
//! full objects) are kept per view, and only the visible window crosses IPC.

pub mod detail;
pub mod rows;
pub mod yaml;

use std::cmp::Ordering;

use serde::{Deserialize, Serialize, Serializer};

/// Resource kinds shown in the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResourceKind {
    Pods,
    Deployments,
    ReplicaSets,
    StatefulSets,
    DaemonSets,
    Services,
    Ingresses,
    ConfigMaps,
    Secrets,
    PersistentVolumeClaims,
    Events,
    Nodes,
    Namespaces,
}

impl ResourceKind {
    pub const ALL: [ResourceKind; 13] = [
        ResourceKind::Pods,
        ResourceKind::Deployments,
        ResourceKind::ReplicaSets,
        ResourceKind::StatefulSets,
        ResourceKind::DaemonSets,
        ResourceKind::Services,
        ResourceKind::Ingresses,
        ResourceKind::ConfigMaps,
        ResourceKind::Secrets,
        ResourceKind::PersistentVolumeClaims,
        ResourceKind::Events,
        ResourceKind::Nodes,
        ResourceKind::Namespaces,
    ];

    pub fn namespaced(self) -> bool {
        !matches!(self, ResourceKind::Nodes | ResourceKind::Namespaces)
    }

    pub fn title(self) -> &'static str {
        match self {
            ResourceKind::Pods => "Pods",
            ResourceKind::Deployments => "Deployments",
            ResourceKind::ReplicaSets => "ReplicaSets",
            ResourceKind::StatefulSets => "StatefulSets",
            ResourceKind::DaemonSets => "DaemonSets",
            ResourceKind::Services => "Services",
            ResourceKind::Ingresses => "Ingresses",
            ResourceKind::ConfigMaps => "ConfigMaps",
            ResourceKind::Secrets => "Secrets",
            ResourceKind::PersistentVolumeClaims => "PersistentVolumeClaims",
            ResourceKind::Events => "Events",
            ResourceKind::Nodes => "Nodes",
            ResourceKind::Namespaces => "Namespaces",
        }
    }

    /// Group shown as a sidebar section header.
    pub fn group(self) -> &'static str {
        match self {
            ResourceKind::Pods
            | ResourceKind::Deployments
            | ResourceKind::ReplicaSets
            | ResourceKind::StatefulSets
            | ResourceKind::DaemonSets => "Workloads",
            ResourceKind::Services | ResourceKind::Ingresses => "Network",
            ResourceKind::ConfigMaps
            | ResourceKind::Secrets
            | ResourceKind::PersistentVolumeClaims => "Config & Storage",
            ResourceKind::Events | ResourceKind::Nodes | ResourceKind::Namespaces => "Cluster",
        }
    }

    /// Type-specific columns (Name and Namespace are added by [`columns_for`]).
    fn specific_columns(self) -> &'static [Column] {
        use ColumnKind::*;
        // A macro (not a const fn) so the slices are promoted to 'static.
        macro_rules! col {
            ($id:literal, $title:literal, $kind:ident, $width:literal) => {
                Column {
                    id: $id,
                    title: $title,
                    kind: $kind,
                    width: $width,
                }
            };
        }
        match self {
            ResourceKind::Pods => &[
                col!("ready", "Ready", Text, 70),
                col!("status", "Status", Status, 150),
                col!("restarts", "Restarts", Number, 80),
                col!("node", "Node", Text, 180),
                col!("ip", "IP", Text, 120),
                col!("age", "Age", Age, 70),
            ],
            ResourceKind::Deployments => &[
                col!("ready", "Ready", Text, 80),
                col!("upToDate", "Up-to-date", Number, 90),
                col!("available", "Available", Number, 90),
                col!("age", "Age", Age, 70),
            ],
            ResourceKind::ReplicaSets => &[
                col!("desired", "Desired", Number, 80),
                col!("current", "Current", Number, 80),
                col!("ready", "Ready", Number, 80),
                col!("age", "Age", Age, 70),
            ],
            ResourceKind::StatefulSets => &[
                col!("ready", "Ready", Text, 80),
                col!("age", "Age", Age, 70),
            ],
            ResourceKind::DaemonSets => &[
                col!("desired", "Desired", Number, 80),
                col!("current", "Current", Number, 80),
                col!("ready", "Ready", Number, 80),
                col!("upToDate", "Up-to-date", Number, 90),
                col!("available", "Available", Number, 90),
                col!("age", "Age", Age, 70),
            ],
            ResourceKind::Services => &[
                col!("type", "Type", Text, 110),
                col!("clusterIp", "Cluster IP", Text, 130),
                col!("externalIp", "External IP", Text, 140),
                col!("ports", "Ports", Text, 180),
                col!("age", "Age", Age, 70),
            ],
            ResourceKind::Ingresses => &[
                col!("class", "Class", Text, 100),
                col!("hosts", "Hosts", Text, 220),
                col!("address", "Address", Text, 140),
                col!("age", "Age", Age, 70),
            ],
            ResourceKind::ConfigMaps => &[
                col!("data", "Data", Number, 70),
                col!("age", "Age", Age, 70),
            ],
            ResourceKind::Secrets => &[
                col!("type", "Type", Text, 220),
                col!("data", "Data", Number, 70),
                col!("age", "Age", Age, 70),
            ],
            ResourceKind::PersistentVolumeClaims => &[
                col!("status", "Status", Status, 90),
                col!("volume", "Volume", Text, 200),
                col!("capacity", "Capacity", Text, 90),
                col!("accessModes", "Access modes", Text, 110),
                col!("storageClass", "Storage class", Text, 130),
                col!("age", "Age", Age, 70),
            ],
            ResourceKind::Events => &[
                col!("lastSeen", "Last seen", Age, 80),
                col!("type", "Type", Status, 80),
                col!("reason", "Reason", Text, 140),
                col!("object", "Object", Text, 220),
                col!("count", "Count", Number, 60),
                col!("message", "Message", Text, 420),
            ],
            ResourceKind::Nodes => &[
                col!("status", "Status", Status, 160),
                col!("roles", "Roles", Text, 140),
                col!("version", "Version", Text, 110),
                col!("internalIp", "Internal IP", Text, 130),
                col!("age", "Age", Age, 70),
            ],
            ResourceKind::Namespaces => &[
                col!("status", "Status", Status, 100),
                col!("age", "Age", Age, 70),
            ],
        }
    }

    /// Column the view sorts by when it opens.
    pub fn default_sort(self) -> SortSpec {
        match self {
            ResourceKind::Events => SortSpec {
                column: "lastSeen".into(),
                descending: false,
            },
            _ => SortSpec {
                column: "name".into(),
                descending: false,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ColumnKind {
    Text,
    Number,
    /// Unix seconds; rendered as a live "age" by the frontend.
    Age,
    /// Text with a semantic tone.
    Status,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Column {
    pub id: &'static str,
    pub title: &'static str,
    pub kind: ColumnKind,
    /// Default width in px.
    pub width: u16,
}

const NAME_COLUMN: Column = Column {
    id: "name",
    title: "Name",
    kind: ColumnKind::Text,
    width: 320,
};
const NAMESPACE_COLUMN: Column = Column {
    id: "namespace",
    title: "Namespace",
    kind: ColumnKind::Text,
    width: 160,
};

/// Full column list for a view. The Namespace column only appears when a
/// namespaced kind is viewed across all namespaces.
pub fn columns_for(kind: ResourceKind, all_namespaces: bool) -> Vec<Column> {
    let mut cols = vec![NAME_COLUMN];
    if kind.namespaced() && all_namespaces {
        cols.push(NAMESPACE_COLUMN);
    }
    cols.extend_from_slice(kind.specific_columns());
    cols
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SortSpec {
    pub column: String,
    pub descending: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Tone {
    Ok,
    Warn,
    Error,
    Muted,
}

/// One table cell.
#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    Text(String),
    Number(i64),
    /// Unix seconds.
    Time(i64),
    Status {
        text: String,
        tone: Tone,
    },
    Empty,
}

impl Cell {
    pub fn text(s: impl Into<String>) -> Cell {
        let s = s.into();
        if s.is_empty() {
            Cell::Empty
        } else {
            Cell::Text(s)
        }
    }

    pub fn status(text: impl Into<String>, tone: Tone) -> Cell {
        Cell::Status {
            text: text.into(),
            tone,
        }
    }

    pub fn time(ts: Option<i64>) -> Cell {
        ts.map(Cell::Time).unwrap_or(Cell::Empty)
    }

    /// Ordering used for sorting; empty cells always sort last.
    pub fn compare(&self, other: &Cell) -> Ordering {
        match (self, other) {
            (Cell::Empty, Cell::Empty) => Ordering::Equal,
            (Cell::Empty, _) => Ordering::Greater,
            (_, Cell::Empty) => Ordering::Less,
            (Cell::Number(a), Cell::Number(b)) => a.cmp(b),
            // Older timestamp = larger age, so ascending age = descending time.
            (Cell::Time(a), Cell::Time(b)) => b.cmp(a),
            (a, b) => natural_cmp(a.sort_text(), b.sort_text()),
        }
    }

    fn sort_text(&self) -> &str {
        match self {
            Cell::Text(s) | Cell::Status { text: s, .. } => s,
            _ => "",
        }
    }
}

impl Serialize for Cell {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct StatusCell<'a> {
            t: &'a str,
            tone: Tone,
        }
        match self {
            Cell::Text(t) => s.serialize_str(t),
            Cell::Number(n) | Cell::Time(n) => s.serialize_i64(*n),
            Cell::Status { text, tone } => StatusCell {
                t: text,
                tone: *tone,
            }
            .serialize(s),
            Cell::Empty => s.serialize_none(),
        }
    }
}

/// Case-insensitive comparison that orders embedded numbers numerically
/// (`pod-2` < `pod-10`), matching how people scan resource names.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.as_bytes(), b.as_bytes());
    loop {
        match (a.first(), b.first()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let da = a.iter().take_while(|c| c.is_ascii_digit()).count();
                let db = b.iter().take_while(|c| c.is_ascii_digit()).count();
                let (na, nb) = (trim_zeros(&a[..da]), trim_zeros(&b[..db]));
                let ord = na.len().cmp(&nb.len()).then_with(|| na.cmp(nb));
                if ord != Ordering::Equal {
                    return ord;
                }
                a = &a[da..];
                b = &b[db..];
            }
            (Some(x), Some(y)) => {
                let ord = x.to_ascii_lowercase().cmp(&y.to_ascii_lowercase());
                if ord != Ordering::Equal {
                    return ord;
                }
                a = &a[1..];
                b = &b[1..];
            }
        }
    }
}

fn trim_zeros(digits: &[u8]) -> &[u8] {
    let start = digits
        .iter()
        .position(|&d| d != b'0')
        .unwrap_or(digits.len());
    &digits[start..]
}

/// A table row. `cells` follows the order of [`columns_for`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub uid: String,
    pub name: String,
    pub namespace: Option<String>,
    pub cells: Vec<Cell>,
    /// Lowercased "namespace/name" used by the text filter.
    #[serde(skip)]
    pub search_key: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order_handles_numbers_and_case() {
        let mut names = vec!["pod-10", "Pod-2", "pod-1", "pod-02b", "api"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(names, ["api", "pod-1", "Pod-2", "pod-02b", "pod-10"]);
    }

    #[test]
    fn empty_cells_sort_last_and_age_sorts_youngest_first() {
        assert_eq!(Cell::Empty.compare(&Cell::Number(1)), Ordering::Greater);
        // Created at t=200 is younger than t=100 → smaller age → sorts first.
        assert_eq!(Cell::Time(200).compare(&Cell::Time(100)), Ordering::Less);
    }

    #[test]
    fn namespace_column_only_for_all_namespaces() {
        let ids = |k, all| columns_for(k, all).iter().map(|c| c.id).collect::<Vec<_>>();
        assert_eq!(
            ids(ResourceKind::ConfigMaps, false),
            ["name", "data", "age"]
        );
        assert_eq!(
            ids(ResourceKind::ConfigMaps, true),
            ["name", "namespace", "data", "age"]
        );
        assert_eq!(ids(ResourceKind::Nodes, true)[1], "status");
    }

    #[test]
    fn cells_serialize_compactly() {
        let json = serde_json::to_string(&vec![
            Cell::text("a"),
            Cell::Number(3),
            Cell::Time(10),
            Cell::status("Running", Tone::Ok),
            Cell::Empty,
        ])
        .unwrap();
        assert_eq!(json, r#"["a",3,10,{"t":"Running","tone":"ok"},null]"#);
    }
}
