//! File-system watching for files changed outside the editor.
//!
//! Language servers that register `workspace/didChangeWatchedFiles` must hear
//! about creates/changes/deletes anywhere in the workspace (git checkout, build
//! tools, other editors), not only for open buffers.
//!
//! Design constraints (rust-analyzer registers watchers for every Rust
//! project, and `target/` can hold hundreds of thousands of directories):
//! - Directories are walked with a `.gitignore`-aware walker plus a hard skip
//!   list, and watched NON-recursively; a directory created later is added as
//!   it appears.
//! - All walking and watch registration happens on a worker thread. The editor
//!   tick only sends commands and drains channels; it never blocks on I/O.
//! - Raw events are filtered by the servers' registered globs before they are
//!   stored, and the pending set is capped.

use crate::lsp::{WatchedChange, WatchedFileEvent};
use ignore::WalkBuilder;
use notify::event::{ModifyKind, RenameMode};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Quiet period after the last event before a batch is released.
const QUIET_PERIOD: Duration = Duration::from_millis(150);
/// A batch is released after this long even if events keep arriving.
const MAX_BATCH_AGE: Duration = Duration::from_secs(1);
/// Most distinct paths held for one batch; beyond this the batch is dropped.
const MAX_PENDING: usize = 10_000;

/// Directory names that are never watched, wherever they appear.
const SKIP_DIRS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    "build",
    "out",
    "dist",
    ".gradle",
    ".idea",
    "__pycache__",
    ".venv",
];

fn is_skipped_name(name: &std::ffi::OsStr) -> bool {
    SKIP_DIRS.iter().any(|skip| name == *skip)
}

#[derive(Default, Clone, Copy)]
struct Seen {
    created: bool,
    removed: bool,
}

enum Command {
    /// Make the watched roots equal to this set.
    Sync(Vec<PathBuf>),
    /// Watch a newly created directory (and its non-ignored subdirectories).
    AddDir(PathBuf),
}

type WatchedSet = Arc<Mutex<HashSet<PathBuf>>>;

/// Owns the `notify` watcher; runs all blocking work.
fn worker(
    commands: Receiver<Command>,
    events: Sender<notify::Result<notify::Event>>,
    errors: Sender<String>,
    watched: WatchedSet,
) {
    let callback_events = events;
    let mut watcher: RecommendedWatcher = match notify::recommended_watcher(move |event| {
        let _ = callback_events.send(event);
    }) {
        Ok(watcher) => watcher,
        Err(error) => {
            let _ = errors.send(format!(
                "cannot watch files for the language server: {error}"
            ));
            return;
        }
    };
    let mut roots: BTreeSet<PathBuf> = BTreeSet::new();

    let watch_tree =
        |watcher: &mut RecommendedWatcher, top: &Path, first_error: &mut Option<String>| {
            let walker = WalkBuilder::new(top)
                .hidden(false)
                .require_git(false)
                .follow_links(false)
                .filter_entry(|entry| entry.depth() == 0 || !is_skipped_name(entry.file_name()))
                .build();
            for entry in walker.flatten() {
                if !entry.file_type().is_some_and(|t| t.is_dir()) {
                    continue;
                }
                let dir = entry.into_path();
                if watched.lock().map(|w| w.contains(&dir)).unwrap_or(false) {
                    continue;
                }
                match watcher.watch(&dir, RecursiveMode::NonRecursive) {
                    Ok(()) => {
                        if let Ok(mut w) = watched.lock() {
                            w.insert(dir);
                        }
                    }
                    Err(error) => {
                        first_error.get_or_insert(format!(
                            "cannot watch {} for the language server: {error}",
                            dir.display()
                        ));
                    }
                }
            }
        };

    while let Ok(command) = commands.recv() {
        let mut error = None;
        match command {
            Command::Sync(wanted) => {
                let wanted: BTreeSet<PathBuf> = wanted.into_iter().collect();
                for stale in roots.difference(&wanted) {
                    if let Ok(mut w) = watched.lock() {
                        let gone: Vec<PathBuf> =
                            w.iter().filter(|d| d.starts_with(stale)).cloned().collect();
                        for dir in gone {
                            let _ = watcher.unwatch(&dir);
                            w.remove(&dir);
                        }
                    }
                }
                for added in wanted.difference(&roots) {
                    watch_tree(&mut watcher, added, &mut error);
                }
                roots = wanted;
            }
            Command::AddDir(dir) => watch_tree(&mut watcher, &dir, &mut error),
        }
        if let Some(message) = error {
            let _ = errors.send(message);
        }
    }
}

#[derive(Default)]
pub struct WorkspaceWatcher {
    commands: Option<Sender<Command>>,
    events: Option<Receiver<notify::Result<notify::Event>>>,
    errors: Option<Receiver<String>>,
    watched: WatchedSet,
    roots: BTreeSet<PathBuf>,
    pending: HashMap<PathBuf, Seen>,
    first_event: Option<Instant>,
    last_event: Option<Instant>,
    /// Last watcher setup error, surfaced once to the user.
    pub last_error: Option<String>,
}

impl WorkspaceWatcher {
    pub fn roots(&self) -> &BTreeSet<PathBuf> {
        &self.roots
    }

    /// Directories currently watched (for diagnostics and tests).
    pub fn watched_dirs(&self) -> HashSet<PathBuf> {
        self.watched.lock().map(|w| w.clone()).unwrap_or_default()
    }

    /// Asks the worker to make the watched roots equal `wanted`. Returns
    /// immediately; setup errors arrive later through [`Self::poll`].
    pub fn sync_roots(&mut self, wanted: &[PathBuf]) {
        let wanted: BTreeSet<PathBuf> = wanted.iter().cloned().collect();
        if wanted == self.roots {
            return;
        }
        if wanted.is_empty() {
            // Dropping the command channel stops the worker and its watcher.
            *self = Self::default();
            return;
        }
        if self.commands.is_none() {
            let (cmd_tx, cmd_rx) = channel();
            let (ev_tx, ev_rx) = channel();
            let (err_tx, err_rx) = channel();
            let watched = self.watched.clone();
            std::thread::Builder::new()
                .name("ovim-workspace-watch".into())
                .spawn(move || worker(cmd_rx, ev_tx, err_tx, watched))
                .ok();
            self.commands = Some(cmd_tx);
            self.events = Some(ev_rx);
            self.errors = Some(err_rx);
        }
        if let Some(commands) = &self.commands {
            let _ = commands.send(Command::Sync(wanted.iter().cloned().collect()));
        }
        self.roots = wanted;
    }

    fn is_ignored(&self, path: &Path) -> bool {
        let relative = self
            .roots
            .iter()
            .find_map(|root| path.strip_prefix(root).ok())
            .unwrap_or(path);
        relative
            .components()
            .any(|part| is_skipped_name(part.as_os_str()))
    }

    fn record(&mut self, event: notify::Event, now: Instant, wanted: &dyn Fn(&Path) -> bool) {
        let created_dirs: Vec<PathBuf> = match event.kind {
            EventKind::Create(_)
            | EventKind::Modify(ModifyKind::Name(RenameMode::To | RenameMode::Both)) => event
                .paths
                .iter()
                .filter(|p| !self.is_ignored(p) && p.is_dir())
                .cloned()
                .collect(),
            _ => Vec::new(),
        };
        if let Some(commands) = &self.commands {
            for dir in created_dirs {
                let _ = commands.send(Command::AddDir(dir));
            }
        }

        let mark = |this: &mut Self, path: &PathBuf, created: bool, removed: bool| {
            if this.is_ignored(path) || !wanted(path) {
                return;
            }
            if this.pending.len() >= MAX_PENDING && !this.pending.contains_key(path) {
                crate::lsp_warn!(
                    "Watcher",
                    "More than {} changed files in one burst; dropping the batch",
                    MAX_PENDING
                );
                this.pending.clear();
                this.first_event = None;
                this.last_event = None;
                return;
            }
            let seen = this.pending.entry(path.clone()).or_default();
            seen.created |= created;
            seen.removed |= removed;
            this.first_event.get_or_insert(now);
            this.last_event = Some(now);
        };
        match event.kind {
            EventKind::Create(_) => event.paths.iter().for_each(|p| mark(self, p, true, false)),
            EventKind::Remove(_) => event.paths.iter().for_each(|p| mark(self, p, false, true)),
            EventKind::Modify(ModifyKind::Name(RenameMode::From)) => {
                event.paths.iter().for_each(|p| mark(self, p, false, true))
            }
            EventKind::Modify(ModifyKind::Name(RenameMode::To)) => {
                event.paths.iter().for_each(|p| mark(self, p, true, false))
            }
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => {
                if let [from, to] = event.paths.as_slice() {
                    mark(self, from, false, true);
                    mark(self, to, true, false);
                }
            }
            EventKind::Modify(_) | EventKind::Any => {
                event.paths.iter().for_each(|p| mark(self, p, false, false))
            }
            EventKind::Access(_) | EventKind::Other => {}
        }
    }

    /// Drains raw events (keeping only paths `wanted` accepts) and returns a
    /// coalesced batch once the burst has settled. Never blocks.
    pub fn poll(
        &mut self,
        now: Instant,
        wanted: &dyn Fn(&Path) -> bool,
    ) -> Option<Vec<WatchedFileEvent>> {
        if let Some(errors) = &self.errors {
            while let Ok(message) = errors.try_recv() {
                self.last_error = Some(message);
            }
        }
        let mut raw = Vec::new();
        if let Some(rx) = &self.events {
            while let Ok(Ok(event)) = rx.try_recv() {
                raw.push(event);
                if raw.len() >= 4096 {
                    break; // keep the tick short; the rest is drained next tick
                }
            }
        }
        for event in raw {
            self.record(event, now, wanted);
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

    /// Takes the last setup error so it is reported once.
    pub fn take_error(&mut self) -> Option<String> {
        self.last_error.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_until(mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(Instant::now() < deadline, "timed out");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn drain(watcher: &mut WorkspaceWatcher, wait: Duration) -> Vec<WatchedFileEvent> {
        let end = Instant::now() + wait;
        let mut all = Vec::new();
        while Instant::now() < end {
            if let Some(batch) = watcher.poll(Instant::now(), &|_| true) {
                all.extend(batch);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        all
    }

    #[test]
    fn skips_target_watches_source_dirs_and_follows_new_directories() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("src/a")).unwrap();
        for n in 0..300 {
            std::fs::create_dir_all(root.join(format!("target/debug/deps/d{n}"))).unwrap();
        }
        std::fs::create_dir_all(root.join("node_modules/x")).unwrap();

        let mut watcher = WorkspaceWatcher::default();
        let started = Instant::now();
        watcher.sync_roots(std::slice::from_ref(&root));
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "sync_roots must not walk on the caller's thread"
        );
        wait_until(|| watcher.watched_dirs().contains(&root.join("src/a")));
        let watched = watcher.watched_dirs();
        assert!(watched.contains(&root) && watched.contains(&root.join("src")));
        assert!(
            watched.iter().all(|d| !d.starts_with(root.join("target"))
                && !d.starts_with(root.join("node_modules"))),
            "skip-listed directories must not be watched: {watched:?}"
        );
        assert!(watched.len() < 10, "{}", watched.len());

        // Changes under target/ produce no events; changes in src do.
        std::fs::write(root.join("target/debug/deps/d1/x.rs"), "x").unwrap();
        std::fs::write(root.join("src/a/lib.rs"), "fn a() {}").unwrap();
        let events = drain(&mut watcher, Duration::from_millis(800));
        assert!(
            events.iter().any(|e| e.path == root.join("src/a/lib.rs")),
            "{events:?}"
        );
        assert!(events
            .iter()
            .all(|e| !e.path.starts_with(root.join("target"))));

        // A directory created later gets its own watch.
        std::fs::create_dir_all(root.join("src/newdir")).unwrap();
        // The tick (poll) is what notices the Create event and asks for the watch.
        wait_until(|| {
            watcher.poll(Instant::now(), &|_| true);
            watcher.watched_dirs().contains(&root.join("src/newdir"))
        });
        std::fs::write(root.join("src/newdir/b.rs"), "fn b() {}").unwrap();
        let events = drain(&mut watcher, Duration::from_millis(800));
        assert!(
            events
                .iter()
                .any(|e| e.path == root.join("src/newdir/b.rs")),
            "{events:?}"
        );
    }

    #[test]
    fn events_are_filtered_by_registered_globs_before_being_stored() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let mut watcher = WorkspaceWatcher::default();
        watcher.sync_roots(std::slice::from_ref(&root));
        wait_until(|| watcher.watched_dirs().contains(&root));
        std::fs::write(root.join("a.rs"), "").unwrap();
        std::fs::write(root.join("a.txt"), "").unwrap();
        let end = Instant::now() + Duration::from_millis(800);
        let mut events = Vec::new();
        while Instant::now() < end {
            if let Some(b) = watcher.poll(Instant::now(), &|p| {
                p.extension().is_some_and(|e| e == "rs")
            }) {
                events.extend(b);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(events.iter().any(|e| e.path.ends_with("a.rs")));
        assert!(events.iter().all(|e| !e.path.ends_with("a.txt")));
    }
}
