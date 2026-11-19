//! Registry of running subscription tasks.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use futures::future::BoxFuture;

use crate::logs::store::LogStore;
use crate::view::store::ViewStore;

/// Data a subscription exposes to window/query commands.
#[derive(Clone, Default)]
pub enum Attachment {
    #[default]
    None,
    Table(Arc<Mutex<ViewStore>>),
    Logs {
        store: Arc<Mutex<LogStore>>,
        containers: Arc<Vec<String>>,
    },
}

struct Entry {
    context: String,
    task: tokio::task::AbortHandle,
    attachment: Attachment,
}

#[derive(Default)]
pub struct Subscriptions {
    next_id: AtomicU64,
    entries: Mutex<HashMap<u64, Entry>>,
}

impl Subscriptions {
    /// Spawns a subscription task and returns its id. The entry removes
    /// itself when the task ends on its own (e.g. the subscriber went away).
    pub fn spawn(
        self: &Arc<Self>,
        context: &str,
        attachment: Attachment,
        task: BoxFuture<'static, ()>,
    ) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let registry: Weak<Self> = Arc::downgrade(self);
        // Hold the lock while spawning so a task that finishes immediately
        // cannot try to remove its entry before it was inserted.
        let mut entries = self.entries.lock().expect("subscriptions poisoned");
        let handle = tokio::spawn(async move {
            task.await;
            if let Some(registry) = registry.upgrade() {
                registry
                    .entries
                    .lock()
                    .expect("subscriptions poisoned")
                    .remove(&id);
            }
        });
        entries.insert(
            id,
            Entry {
                context: context.to_string(),
                task: handle.abort_handle(),
                attachment,
            },
        );
        id
    }

    pub fn close(&self, id: u64) {
        if let Some(entry) = self
            .entries
            .lock()
            .expect("subscriptions poisoned")
            .remove(&id)
        {
            entry.task.abort();
        }
    }

    /// Stops every subscription of a context (on disconnect / context switch).
    pub fn close_context(&self, context: &str) {
        self.entries
            .lock()
            .expect("subscriptions poisoned")
            .retain(|_, e| {
                let keep = e.context != context;
                if !keep {
                    e.task.abort();
                }
                keep
            });
    }

    /// Stops everything (frontend reload).
    pub fn close_all(&self) {
        for (_, entry) in self.entries.lock().expect("subscriptions poisoned").drain() {
            entry.task.abort();
        }
    }

    pub fn attachment(&self, id: u64) -> Attachment {
        self.entries
            .lock()
            .expect("subscriptions poisoned")
            .get(&id)
            .map(|e| e.attachment.clone())
            .unwrap_or_default()
    }

    pub fn view(&self, id: u64) -> Option<Arc<Mutex<ViewStore>>> {
        match self.attachment(id) {
            Attachment::Table(store) => Some(store),
            _ => None,
        }
    }

    /// Number of running subscriptions.
    pub fn active(&self) -> usize {
        self.entries.lock().expect("subscriptions poisoned").len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::FutureExt;
    use std::time::Duration;

    #[tokio::test]
    async fn finished_tasks_remove_themselves() {
        let subs = Arc::new(Subscriptions::default());
        subs.spawn("ctx", Attachment::None, async {}.boxed());
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(subs.active(), 0);
    }

    #[tokio::test]
    async fn close_context_aborts_only_that_context() {
        let subs = Arc::new(Subscriptions::default());
        let a = subs.spawn("a", Attachment::None, futures::future::pending().boxed());
        let b = subs.spawn("b", Attachment::None, futures::future::pending().boxed());
        assert_ne!(a, b);
        subs.close_context("a");
        assert_eq!(subs.active(), 1);
        subs.close(b);
        assert_eq!(subs.active(), 0);
    }
}
