//! Merging the log streams of several containers by timestamp (k-way merge).
//!
//! Each container's log is already in time order, so only the next line of
//! every stream is held: memory stays constant however long the logs are,
//! and merged lines can be consumed as they arrive.

use std::collections::VecDeque;

use super::LineReader;
use super::store::LogLine;

/// A stream that failed mid-way; it is treated as ended from then on.
#[derive(Debug)]
pub struct StreamError {
    /// Container index the stream was opened with.
    pub container: u16,
    pub error: std::io::Error,
}

pub struct Merge<R> {
    /// Container index and reader of every stream.
    readers: Vec<(u16, LineReader<R>)>,
    /// Next line of each stream; `None` once it ended (or failed).
    heads: Vec<Option<LogLine>>,
    primed: bool,
    failures: VecDeque<StreamError>,
}

impl<R: futures::AsyncBufRead + Unpin> Merge<R> {
    /// `readers` pairs each stream with the container index its lines carry.
    pub fn new(readers: Vec<(u16, LineReader<R>)>) -> Self {
        Merge {
            heads: Vec::with_capacity(readers.len()),
            readers,
            primed: false,
            failures: VecDeque::new(),
        }
    }

    /// The oldest pending line across all streams, or the failure of one
    /// stream (which then counts as ended). `None` once every stream ended.
    pub async fn next(&mut self) -> Option<Result<LogLine, StreamError>> {
        if !self.primed {
            self.primed = true;
            for i in 0..self.readers.len() {
                let head = self.read(i).await;
                self.heads.push(head);
            }
        }
        if let Some(failure) = self.failures.pop_front() {
            return Some(Err(failure));
        }
        let i = oldest(&self.heads)?;
        let line = self.heads[i].take().expect("oldest head exists");
        self.heads[i] = self.read(i).await;
        Some(Ok(line))
    }

    async fn read(&mut self, i: usize) -> Option<LogLine> {
        let (container, reader) = &mut self.readers[i];
        match reader.next(*container).await? {
            Ok(line) => Some(line),
            Err(error) => {
                self.failures.push_back(StreamError {
                    container: *container,
                    error,
                });
                None
            }
        }
    }
}

/// Index of the head with the smallest timestamp (ties: lowest index, so
/// container order is stable).
fn oldest(heads: &[Option<LogLine>]) -> Option<usize> {
    heads
        .iter()
        .enumerate()
        .filter_map(|(i, h)| h.as_ref().map(|l| (l.ts, i)))
        .min()
        .map(|(_, i)| i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::TryStreamExt;

    fn line(ts: i64, container: u16, text: &str) -> LogLine {
        LogLine {
            ts,
            container,
            text: text.into(),
        }
    }

    fn cursor(data: &'static [u8]) -> LineReader<futures::io::Cursor<&'static [u8]>> {
        LineReader::new(futures::io::Cursor::new(data))
    }

    async fn drain<R: futures::AsyncBufRead + Unpin>(
        mut merge: Merge<R>,
    ) -> (Vec<(u16, String)>, Vec<u16>) {
        let (mut lines, mut failed) = (Vec::new(), Vec::new());
        while let Some(next) = merge.next().await {
            match next {
                Ok(l) => lines.push((l.container, l.text.to_string())),
                Err(e) => failed.push(e.container),
            }
        }
        (lines, failed)
    }

    #[test]
    fn oldest_picks_smallest_timestamp_with_stable_ties() {
        let heads = vec![
            Some(line(5, 0, "a")),
            None,
            Some(line(3, 2, "c")),
            Some(line(3, 3, "d")),
        ];
        assert_eq!(oldest(&heads), Some(2));
        assert_eq!(oldest(&[None, None]), None);
    }

    #[tokio::test]
    async fn merges_by_time_keeping_container_indices() {
        let merge = Merge::new(vec![
            (
                0,
                cursor(b"2026-11-19T10:00:01Z a1\n2026-11-19T10:00:03Z a3\n"),
            ),
            (
                4,
                cursor(b"2026-11-19T10:00:02Z b2\n2026-11-19T10:00:03Z b3\n"),
            ),
        ]);
        let (lines, failed) = drain(merge).await;
        assert_eq!(
            lines,
            [(0, "a1"), (4, "b2"), (0, "a3"), (4, "b3")].map(|(c, t)| (c, t.to_string()))
        );
        assert!(failed.is_empty());
    }

    #[tokio::test]
    async fn a_failing_stream_is_reported_once_and_the_others_continue() {
        let broken = futures::stream::iter(vec![
            Ok(&b"2026-11-19T10:00:01Z x1\n"[..]),
            Err(std::io::Error::other("connection reset")),
        ])
        .into_async_read();
        let merge = Merge::new(vec![
            (
                0,
                LineReader::new(Box::pin(broken) as std::pin::Pin<Box<dyn futures::AsyncBufRead>>),
            ),
            (
                1,
                LineReader::new(Box::pin(futures::io::Cursor::new(
                    &b"2026-11-19T10:00:00Z y0\n2026-11-19T10:00:02Z y2\n"[..],
                ))
                    as std::pin::Pin<Box<dyn futures::AsyncBufRead>>),
            ),
        ]);
        let (lines, failed) = drain(merge).await;
        assert_eq!(
            lines,
            [(1, "y0"), (0, "x1"), (1, "y2")].map(|(c, t)| (c, t.to_string()))
        );
        assert_eq!(failed, [0]);
    }

    #[tokio::test]
    async fn no_streams_is_empty() {
        let merge: Merge<futures::io::Cursor<&[u8]>> = Merge::new(Vec::new());
        assert_eq!(drain(merge).await, (Vec::new(), Vec::new()));
    }
}
