//! Row storage for one table view with incremental sorted/filtered order.
//!
//! The order is a total order (sort cell, namespace, name, uid), so every
//! row's position can be found by binary search. Watch events therefore cost
//! O(log n) plus a memmove instead of a full re-sort, which keeps 100k-row
//! views responsive under churn. A full rebuild only happens when the sort
//! or filter changes, or after a re-list.

use std::cmp::Ordering;
use std::collections::HashMap;

use crate::resources::{Column, Row, SortSpec, natural_cmp};

pub struct ViewStore {
    columns: Vec<Column>,
    rows: Vec<Row>,
    index: HashMap<String, usize>,
    /// Indices into `rows` of rows passing the filter, in display order.
    order: Vec<u32>,
    sort: SortSpec,
    sort_col: usize,
    filter: String,
    version: u64,
}

impl ViewStore {
    pub fn new(columns: Vec<Column>, sort: SortSpec, filter: &str) -> Self {
        let mut store = ViewStore {
            columns,
            rows: Vec::new(),
            index: HashMap::new(),
            order: Vec::new(),
            sort: sort.clone(),
            sort_col: 0,
            filter: String::new(),
            version: 0,
        };
        store.set_query(sort, filter);
        store
    }

    pub fn columns(&self) -> &[Column] {
        &self.columns
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    /// Rows passing the filter.
    pub fn total(&self) -> usize {
        self.order.len()
    }

    /// All rows, ignoring the filter.
    pub fn unfiltered(&self) -> usize {
        self.rows.len()
    }

    pub fn sort(&self) -> &SortSpec {
        &self.sort
    }

    fn cmp_rows(&self, a: &Row, b: &Row) -> Ordering {
        let primary = match (a.cells.get(self.sort_col), b.cells.get(self.sort_col)) {
            (Some(x), Some(y)) => x.compare(y),
            _ => Ordering::Equal,
        };
        let primary = if self.sort.descending {
            primary.reverse()
        } else {
            primary
        };
        primary
            .then_with(|| {
                natural_cmp(
                    a.namespace.as_deref().unwrap_or(""),
                    b.namespace.as_deref().unwrap_or(""),
                )
            })
            .then_with(|| natural_cmp(&a.name, &b.name))
            .then_with(|| a.uid.cmp(&b.uid))
    }

    fn matches(&self, row: &Row) -> bool {
        self.filter.is_empty() || row.search_key.contains(&self.filter)
    }

    /// Position of `row` (which must be in `order`) found by binary search.
    fn position(&self, row: &Row) -> Result<usize, usize> {
        self.order
            .binary_search_by(|&i| self.cmp_rows(&self.rows[i as usize], row))
    }

    fn insert_into_order(&mut self, idx: usize) {
        if !self.matches(&self.rows[idx]) {
            return;
        }
        let pos = match self.position(&self.rows[idx]) {
            Ok(p) | Err(p) => p,
        };
        self.order.insert(pos, idx as u32);
    }

    fn remove_from_order(&mut self, idx: usize) {
        if !self.matches(&self.rows[idx]) {
            return;
        }
        if let Ok(pos) = self.position(&self.rows[idx]) {
            self.order.remove(pos);
        }
    }

    pub fn upsert(&mut self, row: Row) {
        match self.index.get(&row.uid).copied() {
            None => {
                let idx = self.rows.len();
                self.index.insert(row.uid.clone(), idx);
                self.rows.push(row);
                self.insert_into_order(idx);
            }
            Some(idx) => {
                let old = &self.rows[idx];
                let same_position = self.matches(old) == self.matches(&row)
                    && self.cmp_rows(old, &row) == Ordering::Equal;
                if same_position {
                    self.rows[idx] = row;
                } else {
                    self.remove_from_order(idx);
                    self.rows[idx] = row;
                    self.insert_into_order(idx);
                }
            }
        }
        self.version += 1;
    }

    pub fn remove(&mut self, uid: &str) {
        let Some(idx) = self.index.remove(uid) else {
            return;
        };
        self.remove_from_order(idx);
        let last = self.rows.len() - 1;
        if idx != last {
            // `swap_remove` moves the last row into `idx`; repoint its order entry.
            if self.matches(&self.rows[last])
                && let Ok(pos) = self.position(&self.rows[last])
            {
                self.order[pos] = idx as u32;
            }
            self.index.insert(self.rows[last].uid.clone(), idx);
        }
        self.rows.swap_remove(idx);
        self.version += 1;
    }

    /// Replaces the whole content (after a list / re-list).
    pub fn replace_all(&mut self, rows: Vec<Row>) {
        let mut index = HashMap::with_capacity(rows.len());
        let mut unique: Vec<Row> = Vec::with_capacity(rows.len());
        for row in rows {
            match index.get(&row.uid) {
                // The API server never returns duplicate UIDs; keep the latest defensively.
                Some(&i) => unique[i] = row,
                None => {
                    index.insert(row.uid.clone(), unique.len());
                    unique.push(row);
                }
            }
        }
        self.rows = unique;
        self.index = index;
        self.rebuild_order();
        self.version += 1;
    }

    /// Changes sort and filter. Returns true if anything changed.
    pub fn set_query(&mut self, sort: SortSpec, filter: &str) -> bool {
        let filter = filter.trim().to_lowercase();
        let sort_col = self
            .columns
            .iter()
            .position(|c| c.id == sort.column)
            .unwrap_or(0);
        let sort = SortSpec {
            column: self.columns[sort_col].id.to_string(),
            descending: sort.descending,
        };
        if sort == self.sort && filter == self.filter && sort_col == self.sort_col {
            return false;
        }
        self.sort = sort;
        self.sort_col = sort_col;
        self.filter = filter;
        self.rebuild_order();
        self.version += 1;
        true
    }

    fn rebuild_order(&mut self) {
        let mut order: Vec<u32> = (0..self.rows.len() as u32)
            .filter(|&i| self.matches(&self.rows[i as usize]))
            .collect();
        order.sort_unstable_by(|&a, &b| {
            self.cmp_rows(&self.rows[a as usize], &self.rows[b as usize])
        });
        self.order = order;
    }

    /// Rows `[start, end)` of the current order (clamped).
    pub fn window(&self, start: usize, end: usize) -> Vec<Row> {
        let end = end.min(self.order.len());
        let start = start.min(end);
        self.order[start..end]
            .iter()
            .map(|&i| self.rows[i as usize].clone())
            .collect()
    }

    /// Display index of a row, if it passes the filter.
    pub fn index_of(&self, uid: &str) -> Option<usize> {
        let idx = *self.index.get(uid)?;
        if !self.matches(&self.rows[idx]) {
            return None;
        }
        self.position(&self.rows[idx]).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::{Cell, ResourceKind, columns_for};

    fn row(uid: &str, ns: &str, name: &str, data: i64) -> Row {
        Row {
            uid: uid.into(),
            name: name.into(),
            namespace: Some(ns.into()),
            cells: vec![
                Cell::text(name),
                Cell::text(ns),
                Cell::Number(data),
                Cell::Time(0),
            ],
            search_key: format!("{name}\u{1f}{ns}"),
        }
    }

    fn store(sort: &str, desc: bool) -> ViewStore {
        ViewStore::new(
            columns_for(ResourceKind::ConfigMaps, true),
            SortSpec {
                column: sort.into(),
                descending: desc,
            },
            "",
        )
    }

    fn names(s: &ViewStore) -> Vec<String> {
        s.window(0, usize::MAX)
            .into_iter()
            .map(|r| format!("{}/{}", r.namespace.unwrap(), r.name))
            .collect()
    }

    /// Order maintained incrementally must equal a full rebuild.
    fn assert_consistent(s: &mut ViewStore) {
        let incremental = names(s);
        let (sort, filter) = (s.sort.clone(), s.filter.clone());
        s.rebuild_order();
        assert_eq!(incremental, names(s));
        s.set_query(sort, &filter);
        for (pos, &i) in s.order.iter().enumerate() {
            assert_eq!(s.index_of(&s.rows[i as usize].uid), Some(pos));
        }
    }

    #[test]
    fn same_name_in_different_namespaces_are_distinct_rows() {
        let mut s = store("name", false);
        s.upsert(row("u1", "a", "kube-root-ca.crt", 1));
        s.upsert(row("u2", "b", "kube-root-ca.crt", 1));
        assert_eq!(s.total(), 2);
        s.remove("u1");
        assert_eq!(names(&s), ["b/kube-root-ca.crt"]);
    }

    #[test]
    fn incremental_updates_match_full_sort() {
        let mut s = store("data", true);
        for i in 0..200 {
            s.upsert(row(
                &format!("u{i}"),
                &format!("ns{}", i % 7),
                &format!("cm-{}", (i * 37) % 101),
                i % 13,
            ));
        }
        assert_consistent(&mut s);
        // Change sort keys, delete from the middle and the end, re-add.
        for i in (0..200).step_by(3) {
            s.upsert(row(
                &format!("u{i}"),
                &format!("ns{}", i % 7),
                &format!("cm-{}", (i * 37) % 101),
                (i * 5) % 17,
            ));
        }
        for i in (0..200).step_by(4) {
            s.remove(&format!("u{i}"));
        }
        s.remove("u199");
        s.upsert(row("new", "ns0", "aaa", 100));
        assert_consistent(&mut s);
        assert_eq!(s.unfiltered(), 200 - 50 - 1 + 1);
    }

    #[test]
    fn filter_applies_to_inserts_and_updates() {
        let mut s = store("name", false);
        s.set_query(
            SortSpec {
                column: "name".into(),
                descending: false,
            },
            "  API ",
        );
        s.upsert(row("1", "prod", "api-config", 1));
        s.upsert(row("2", "prod", "web-config", 1));
        assert_eq!(s.total(), 1);
        // Renamed so it no longer matches (cannot happen in k8s, but the store must stay consistent).
        s.upsert(row("1", "prod", "other", 1));
        assert_eq!(s.total(), 0);
        s.upsert(row("2", "prod", "api-web", 1));
        assert_eq!(names(&s), ["prod/api-web"]);
        assert_consistent(&mut s);
    }

    #[test]
    fn replace_all_drops_rows_missing_from_relist() {
        let mut s = store("name", false);
        s.upsert(row("1", "a", "x", 1));
        s.upsert(row("2", "a", "y", 1));
        s.replace_all(vec![row("2", "a", "y", 1), row("3", "a", "z", 1)]);
        assert_eq!(names(&s), ["a/y", "a/z"]);
        assert_eq!(s.index_of("1"), None);
    }

    #[test]
    fn unknown_sort_column_falls_back_to_name_and_window_clamps() {
        let mut s = store("nope", false);
        assert_eq!(s.sort().column, "name");
        s.upsert(row("1", "a", "b", 1));
        s.upsert(row("2", "a", "a", 1));
        assert_eq!(s.window(1, 10).len(), 1);
        assert!(s.window(5, 10).is_empty());
    }

    #[test]
    fn version_bumps_on_every_change_but_not_on_noop_query() {
        let mut s = store("name", false);
        let v0 = s.version();
        s.upsert(row("1", "a", "b", 1));
        assert!(s.version() > v0);
        let v1 = s.version();
        assert!(!s.set_query(
            SortSpec {
                column: "name".into(),
                descending: false
            },
            ""
        ));
        assert_eq!(s.version(), v1);
    }
}
