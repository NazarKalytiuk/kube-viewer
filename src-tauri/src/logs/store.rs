//! Bounded in-memory log storage with search.
//!
//! Lines are numbered from 1 for the whole session; when the memory budget
//! is exceeded the oldest lines are dropped in chunks, numbering continues.
//! Search matches are kept as line numbers and maintained incrementally on
//! append, so "filter" views and "find next" stay O(log n) on millions of lines.

use std::collections::VecDeque;

use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

/// Default memory budget for one log view (text + per-line overhead).
pub const DEFAULT_MAX_BYTES: usize = 256 * 1024 * 1024;
const LINE_OVERHEAD: usize = std::mem::size_of::<LogLine>();

#[derive(Debug, Clone)]
pub struct LogLine {
    /// Nanoseconds since the epoch, or [`super::timefmt::NO_TS`].
    pub ts: i64,
    /// Index into the session's container list.
    pub container: u16,
    pub text: Box<str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum QueryMode {
    /// Show only matching lines.
    Filter,
    /// Show all lines; matches are navigated with find next/previous.
    Find,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogQuery {
    pub text: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub mode: QueryMode,
}

struct CompiledQuery {
    regex: Regex,
    mode: QueryMode,
}

impl LogQuery {
    fn compile(&self) -> AppResult<Option<CompiledQuery>> {
        if self.text.is_empty() {
            return Ok(None);
        }
        let pattern = if self.regex {
            self.text.clone()
        } else {
            regex::escape(&self.text)
        };
        let regex = RegexBuilder::new(&pattern)
            .case_insensitive(!self.case_sensitive)
            .size_limit(10 * 1024 * 1024)
            .build()
            .map_err(|e| AppError::InvalidRequest(format!("invalid regular expression: {e}")))?;
        Ok(Some(CompiledQuery {
            regex,
            mode: self.mode,
        }))
    }
}

pub struct LogStore {
    lines: VecDeque<LogLine>,
    /// Number of `lines[0]`.
    first_number: u64,
    bytes: usize,
    max_bytes: usize,
    query: Option<CompiledQuery>,
    /// Line numbers of matching lines, ascending.
    matches: VecDeque<u64>,
    version: u64,
}

impl Default for LogStore {
    fn default() -> Self {
        LogStore::with_budget(DEFAULT_MAX_BYTES)
    }
}

impl LogStore {
    pub fn with_budget(max_bytes: usize) -> Self {
        LogStore {
            lines: VecDeque::new(),
            first_number: 1,
            bytes: 0,
            max_bytes,
            query: None,
            matches: VecDeque::new(),
            version: 0,
        }
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    /// Lines held in memory.
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Lines dropped from the front to stay within the budget.
    pub fn dropped(&self) -> u64 {
        self.first_number - 1
    }

    pub fn match_count(&self) -> usize {
        self.matches.len()
    }

    fn filtering(&self) -> bool {
        matches!(&self.query, Some(q) if q.mode == QueryMode::Filter)
    }

    /// Lines shown by the view (matching lines when filtering).
    pub fn visible_count(&self) -> usize {
        if self.filtering() {
            self.matches.len()
        } else {
            self.lines.len()
        }
    }

    /// Timestamp of the newest line of `container`, used to resume streams.
    pub fn last_ts(&self, container: u16) -> Option<i64> {
        self.lines
            .iter()
            .rev()
            .find(|l| l.container == container && l.ts != super::timefmt::NO_TS)
            .map(|l| l.ts)
    }

    pub fn push(&mut self, line: LogLine) {
        let number = self.first_number + self.lines.len() as u64;
        if let Some(q) = &self.query
            && q.regex.is_match(&line.text)
        {
            self.matches.push_back(number);
        }
        self.bytes += line.text.len() + LINE_OVERHEAD;
        self.lines.push_back(line);
        self.version += 1;
        if self.bytes > self.max_bytes {
            self.evict();
        }
    }

    pub fn extend(&mut self, lines: impl IntoIterator<Item = LogLine>) {
        for line in lines {
            self.push(line);
        }
    }

    /// Drops the oldest ~10% of the budget at once so eviction is amortized.
    fn evict(&mut self) {
        let target = self.max_bytes - self.max_bytes / 10;
        while self.bytes > target {
            let Some(line) = self.lines.pop_front() else {
                break;
            };
            self.bytes -= line.text.len() + LINE_OVERHEAD;
            self.first_number += 1;
        }
        while self.matches.front().is_some_and(|&n| n < self.first_number) {
            self.matches.pop_front();
        }
    }

    /// Sets (or clears) the search. Returns an error for an invalid regex,
    /// leaving the previous query in place.
    pub fn set_query(&mut self, query: Option<&LogQuery>) -> AppResult<()> {
        let compiled = match query {
            Some(q) => q.compile()?,
            None => None,
        };
        self.matches.clear();
        if let Some(q) = &compiled {
            for (i, line) in self.lines.iter().enumerate() {
                if q.regex.is_match(&line.text) {
                    self.matches.push_back(self.first_number + i as u64);
                }
            }
        }
        self.query = compiled;
        self.version += 1;
        Ok(())
    }

    /// Line numbers currently visible (a snapshot for chunked export).
    pub fn visible_numbers(&self) -> Vec<u64> {
        if self.filtering() {
            self.matches.iter().copied().collect()
        } else {
            (self.first_number..self.first_number + self.lines.len() as u64).collect()
        }
    }

    pub fn line_by_number(&self, number: u64) -> Option<&LogLine> {
        self.lines
            .get(number.checked_sub(self.first_number)? as usize)
    }

    /// Visible lines `[start, end)` with their line numbers.
    /// Visible index range of the newest `count` visible lines.
    pub fn tail(&self, count: usize) -> std::ops::Range<usize> {
        let end = self.visible_count();
        end.saturating_sub(count)..end
    }

    pub fn window(&self, start: usize, end: usize) -> Vec<(u64, &LogLine)> {
        let end = end.min(self.visible_count());
        let start = start.min(end);
        if self.filtering() {
            self.matches
                .range(start..end)
                .filter_map(|&n| self.line_by_number(n).map(|l| (n, l)))
                .collect()
        } else {
            self.lines
                .range(start..end)
                .enumerate()
                .map(|(i, l)| (self.first_number + (start + i) as u64, l))
                .collect()
        }
    }

    /// Visible index of a line number.
    fn visible_index(&self, number: u64) -> Option<usize> {
        if self.filtering() {
            self.matches.binary_search(&number).ok()
        } else {
            number
                .checked_sub(self.first_number)
                .map(|i| i as usize)
                .filter(|&i| i < self.lines.len())
        }
    }

    /// Next match strictly after (or before) visible index `from`, wrapping
    /// around. Returns (visible index, 1-based match ordinal).
    pub fn find(&self, from: Option<usize>, forward: bool) -> Option<(usize, usize)> {
        if self.matches.is_empty() {
            return None;
        }
        let from_number = from.and_then(|i| {
            if self.filtering() {
                self.matches.get(i).copied()
            } else {
                Some(self.first_number + i as u64)
            }
        });
        let ordinal = match (from_number, forward) {
            (None, true) => 0,
            (None, false) => self.matches.len() - 1,
            (Some(n), true) => match self.matches.binary_search(&n) {
                Ok(i) => (i + 1) % self.matches.len(),
                Err(i) => i % self.matches.len(),
            },
            (Some(n), false) => match self.matches.binary_search(&n) {
                Ok(0) | Err(0) => self.matches.len() - 1,
                Ok(i) | Err(i) => i - 1,
            },
        };
        let number = self.matches[ordinal];
        self.visible_index(number).map(|idx| (idx, ordinal + 1))
    }

    /// All visible lines, for "save what is loaded".
    pub fn visible_iter(&self) -> Box<dyn Iterator<Item = &LogLine> + '_> {
        if self.filtering() {
            Box::new(self.matches.iter().filter_map(|&n| self.line_by_number(n)))
        } else {
            Box::new(self.lines.iter())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str) -> LogLine {
        LogLine {
            ts: 0,
            container: 0,
            text: text.into(),
        }
    }

    fn texts(store: &LogStore, start: usize, end: usize) -> Vec<(u64, String)> {
        store
            .window(start, end)
            .into_iter()
            .map(|(n, l)| (n, l.text.to_string()))
            .collect()
    }

    fn query(text: &str, mode: QueryMode) -> LogQuery {
        LogQuery {
            text: text.into(),
            regex: false,
            case_sensitive: false,
            mode,
        }
    }

    #[test]
    fn tail_is_the_newest_visible_lines() {
        let mut s = LogStore::default();
        assert_eq!(s.tail(10), 0..0);
        s.extend(["INFO a", "ERROR b", "info c", "error d"].map(line));
        assert_eq!(s.tail(2), 2..4);
        assert_eq!(s.tail(10), 0..4);
        s.set_query(Some(&query("error", QueryMode::Filter)))
            .unwrap();
        assert_eq!(s.tail(1), 1..2);
        let tail = s.tail(1);
        assert_eq!(
            texts(&s, tail.start, tail.end),
            [(4, "error d".to_string())]
        );
    }

    #[test]
    fn filter_shows_only_matches_and_tracks_appends() {
        let mut s = LogStore::default();
        s.extend(["INFO a", "ERROR b", "info c"].map(line));
        s.set_query(Some(&query("error", QueryMode::Filter)))
            .unwrap();
        assert_eq!(texts(&s, 0, 10), [(2, "ERROR b".to_string())]);
        s.push(line("error d"));
        assert_eq!(s.visible_count(), 2);
        assert_eq!(texts(&s, 1, 2), [(4, "error d".to_string())]);
        s.set_query(None).unwrap();
        assert_eq!(s.visible_count(), 4);
    }

    #[test]
    fn find_navigates_and_wraps() {
        let mut s = LogStore::default();
        s.extend(["a", "x1", "b", "x2", "c", "x3"].map(line));
        s.set_query(Some(&query("x", QueryMode::Find))).unwrap();
        assert_eq!(s.match_count(), 3);
        assert_eq!(s.find(None, true), Some((1, 1)));
        assert_eq!(s.find(Some(1), true), Some((3, 2)));
        assert_eq!(
            s.find(Some(2), true),
            Some((3, 2)),
            "from a non-matching line"
        );
        assert_eq!(s.find(Some(5), true), Some((1, 1)), "wraps forward");
        assert_eq!(s.find(Some(1), false), Some((5, 3)), "wraps backward");
        assert_eq!(s.find(None, false), Some((5, 3)));
    }

    #[test]
    fn find_inside_filtered_view_uses_visible_indices() {
        let mut s = LogStore::default();
        s.extend(["a", "x1", "b", "x2"].map(line));
        s.set_query(Some(&query("x", QueryMode::Filter))).unwrap();
        assert_eq!(s.find(Some(0), true), Some((1, 2)));
    }

    #[test]
    fn regex_case_and_invalid_patterns() {
        let mut s = LogStore::default();
        s.extend(["status=500", "status=200", "Status=503"].map(line));
        let mut q = query(r"status=5\d\d", QueryMode::Filter);
        q.regex = true;
        s.set_query(Some(&q)).unwrap();
        assert_eq!(s.visible_count(), 2);
        q.case_sensitive = true;
        s.set_query(Some(&q)).unwrap();
        assert_eq!(s.visible_count(), 1);
        q.text = "(".into();
        assert!(matches!(
            s.set_query(Some(&q)),
            Err(AppError::InvalidRequest(_))
        ));
        assert_eq!(s.visible_count(), 1, "previous query stays active");
    }

    #[test]
    fn literal_mode_escapes_regex_characters() {
        let mut s = LogStore::default();
        s.extend(["a.b", "axb"].map(line));
        s.set_query(Some(&query("a.b", QueryMode::Filter))).unwrap();
        assert_eq!(s.visible_count(), 1);
    }

    #[test]
    fn evicts_oldest_lines_and_keeps_numbering() {
        let per_line = 10 + LINE_OVERHEAD;
        let mut s = LogStore::with_budget(per_line * 100);
        s.set_query(Some(&query("7", QueryMode::Find))).unwrap();
        for i in 0..150 {
            s.push(line(&format!("line-{i:05}")));
        }
        assert!(s.len() <= 100);
        assert_eq!(s.dropped() + s.len() as u64, 150);
        let (first, text) = texts(&s, 0, 1).remove(0);
        assert_eq!(first, s.dropped() + 1);
        assert_eq!(text, format!("line-{:05}", first - 1));
        // Matches before the window were dropped too.
        let first_match = s.find(None, true).unwrap().0;
        assert!(texts(&s, first_match, first_match + 1)[0].1.contains('7'));
    }

    #[test]
    fn last_ts_per_container() {
        let mut s = LogStore::default();
        s.push(LogLine {
            ts: 10,
            container: 0,
            text: "a".into(),
        });
        s.push(LogLine {
            ts: 20,
            container: 1,
            text: "b".into(),
        });
        s.push(LogLine {
            ts: super::super::timefmt::NO_TS,
            container: 0,
            text: "c".into(),
        });
        assert_eq!(s.last_ts(0), Some(10));
        assert_eq!(s.last_ts(1), Some(20));
        assert_eq!(s.last_ts(2), None);
    }
}

#[cfg(test)]
mod perf {
    use super::*;
    use std::time::Instant;

    /// `cargo test --release --lib logs::store::perf -- --ignored --nocapture`
    #[test]
    #[ignore = "perf measurement"]
    fn million_lines() {
        let mut s = LogStore::default();
        let t = Instant::now();
        for i in 0..1_000_000u64 {
            let text = if i % 1000 == 0 {
                format!("2026 ERROR request {i} failed: upstream timeout after 30s")
            } else {
                format!("2026 INFO GET /api/orders/{i} 200 12ms user=alice trace=abc{i}")
            };
            s.push(LogLine {
                ts: i as i64,
                container: 0,
                text: text.into(),
            });
        }
        println!(
            "push 1M lines: {:?} ({} in memory, {} dropped)",
            t.elapsed(),
            s.len(),
            s.dropped()
        );

        let t = Instant::now();
        s.set_query(Some(&LogQuery {
            text: "error".into(),
            regex: false,
            case_sensitive: false,
            mode: QueryMode::Filter,
        }))
        .unwrap();
        println!(
            "filter 1M lines: {:?} → {} matches",
            t.elapsed(),
            s.match_count()
        );

        let t = Instant::now();
        s.set_query(Some(&LogQuery {
            text: r"orders/\d+5 200".into(),
            regex: true,
            case_sensitive: false,
            mode: QueryMode::Find,
        }))
        .unwrap();
        println!(
            "regex find over 1M lines: {:?} → {} matches",
            t.elapsed(),
            s.match_count()
        );

        let t = Instant::now();
        for i in 0..1000 {
            assert!(s.find(Some(i * 900), true).is_some());
        }
        println!("1000 find-next: {:?}", t.elapsed());

        let t = Instant::now();
        for i in 0..1000 {
            assert_eq!(s.window(i * 900, i * 900 + 120).len(), 120);
        }
        println!("1000 window reads (120 lines): {:?}", t.elapsed());
    }
}
