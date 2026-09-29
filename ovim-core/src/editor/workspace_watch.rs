//! File-system watching for files changed outside the editor.
//!
//! Language servers that register `workspace/didChangeWatchedFiles` must hear
//! about creates/changes/deletes anywhere in the workspace (git checkout, build
//! tools, other editors), not only for open buffers. This wraps a recursive
//! `notify` watcher over the roots the LSP manager asks for and coalesces
//! event bursts (a checkout touches hundreds of files) into one batch.

use crate::lsp::{WatchedChange, WatchedFileEvent};
use notify::event::{ModifyKind, RenameMode};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

/// Quiet period after the last event before a batch is released.
const QUIET_PERIOD: Duration = Duration::from_millis(150);
/// A batch is released after this long even if events keep arriving.
const MAX_BATCH_AGE: Duration = Duration::from_secs(1);

#[derive(Default, Clone, Copy)]
struct Seen {
    created: bool,
    removed: bool,
}

#[derive(Default)]
pub struct WorkspaceWatcher {
    watcher: Option<RecommendedWatcher>,
    rx: Option<Receiver<notify::Result<notify::Event>>>,
    roots: BTreeSet<PathBuf>,
    pending: HashMap<PathBuf, Seen>,
    first_event: Option<Instant>,
    last_event: Option<Instant>,
    /// Last watcher setup error, surfaced once to the user.
    pub last_error: Option<String>,
}

fn is_ignored(path: &Path) -> bool {
    path.components().any(|part| part.as_os_str() == ".git")
}

impl WorkspaceWatcher {
    pub fn roots(&self) -> &BTreeSet<PathBuf> {
        &self.roots
    }

    /// Makes the set of watched roots equal to `wanted`. Returns an error
    /// message when a root could not be watched (e.g. inotify watch limit).
    pub fn sync_roots(&mut self, wanted: &[PathBuf]) -> Option<String> {
        let wanted: BTreeSet<PathBuf> = wanted.iter().cloned().collect();
        if wanted == self.roots {
            return None;
        }
        if wanted.is_empty() {
            *self = Self::default();
            return None;
        }
        if self.watcher.is_none() {
            let (tx, rx) = channel();
            match notify::recommended_watcher(move |event| {
                let _ = tx.send(event);
            }) {
                Ok(watcher) => {
                    self.watcher = Some(watcher);
                    self.rx = Some(rx);
                }
                Err(error) => {
                    let message = format!("cannot watch files for the language server: {error}");
                    self.last_error = Some(message.clone());
                    // Remember the roots so we do not retry every tick.
                    self.roots = wanted;
                    return Some(message);
                }
            }
        }
        let watcher = self.watcher.as_mut().expect("watcher created above");
        let mut error = None;
        for stale in self.roots.difference(&wanted) {
            let _ = watcher.unwatch(stale);
        }
        for added in wanted.difference(&self.roots) {
            if let Err(e) = watcher.watch(added, RecursiveMode::Recursive) {
                error = Some(format!(
                    "cannot watch {} for the language server: {e}",
                    added.display()
                ));
            }
        }
        self.roots = wanted;
        if let Some(message) = &error {
            self.last_error = Some(message.clone());
        }
        error
    }

    fn record(&mut self, event: notify::Event, now: Instant) {
        let mut mark = |path: &PathBuf, created: bool, removed: bool| {
            if is_ignored(path) {
                return;
            }
            let seen = self.pending.entry(path.clone()).or_default();
            seen.created |= created;
            seen.removed |= removed;
            self.first_event.get_or_insert(now);
            self.last_event = Some(now);
        };
        match event.kind {
            EventKind::Create(_) => event.paths.iter().for_each(|p| mark(p, true, false)),
            EventKind::Remove(_) => event.paths.iter().for_each(|p| mark(p, false, true)),
            EventKind::Modify(ModifyKind::Name(RenameMode::From)) => {
                event.paths.iter().for_each(|p| mark(p, false, true))
            }
            EventKind::Modify(ModifyKind::Name(RenameMode::To)) => {
                event.paths.iter().for_each(|p| mark(p, true, false))
            }
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => {
                if let [from, to] = event.paths.as_slice() {
                    mark(from, false, true);
                    mark(to, true, false);
                }
            }
            EventKind::Modify(_) | EventKind::Any => {
                event.paths.iter().for_each(|p| mark(p, false, false))
            }
            EventKind::Access(_) | EventKind::Other => {}
        }
    }

    /// Drains raw events and returns a coalesced batch once the burst has
    /// settled.
    pub fn poll(&mut self, now: Instant) -> Option<Vec<WatchedFileEvent>> {
        let mut raw = Vec::new();
        if let Some(rx) = &self.rx {
            while let Ok(event) = rx.try_recv() {
                if let Ok(event) = event {
                    raw.push(event);
                }
            }
        }
        for event in raw {
            self.record(event, now);
        }
        let (first, last) = (self.first_event?, self.last_event?);
        if now.duration_since(last) < QUIET_PERIOD && now.duration_since(first) < MAX_BATCH_AGE {
            return None;
        }
        self.first_event = None;
        self.last_event = None;
        let mut events: Vec<WatchedFileEvent> = std::mem::take(&mut self.pending)
            .into_iter()
            .filter_map(|(path, seen)| {
                let exists = path.exists();
                if exists && path.is_dir() {
                    return None;
                }
                let change = if !exists {
                    WatchedChange::Deleted
                } else if seen.created && !seen.removed {
                    WatchedChange::Created
                } else {
                    WatchedChange::Changed
                };
                Some(WatchedFileEvent { path, change })
            })
            .collect();
        events.sort_by(|a, b| a.path.cmp(&b.path));
        (!events.is_empty()).then_some(events)
    }
}
