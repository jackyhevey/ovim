//! Run / debug launch flow: the editor-side state machine.
//!
//! One launch request moves through these stages, none of which ever block
//! the tick loop (everything slow happens in tasks or child processes that
//! are polled with `try_recv`):
//!
//! ```text
//! Resolving --> Building --> Running            (Run)
//!                        \-> StartingDebugger --> Debugging          (Debug, main)
//! Resolving --> Running                          (Run tests / tasks)
//! Resolving --> AwaitingDebugPort --> StartingDebugger --> Debugging (Debug tests / tasks)
//! ```
//!
//! *Resolving* asks the language server what to launch at the cursor
//! (`hyperion.resolveLaunch`), falling back to `.ovim/debug.toml` and
//! `hyperion.runConfigurations` when it has no answer. Every source ends up
//! as a [`LaunchPlan`]. Output of every stage streams into the run console.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use super::Editor;
use crate::dap::{DapLaunchRequest, PendingDebugAction};
use crate::debug_config::DebugRunConfig;
use crate::launch::console::{LineKind, RunOutcome, RunPhase, RunStatus};
use crate::launch::lsp::{self, ResolveOutcome, ResolveResult};
use crate::launch::plan::{self, LaunchMode, LaunchPlan, PlanKind};
use crate::launch::process::{CommandSpec, ExitInfo, ProcEvent, ProcessHandle, StreamKind};
use crate::launch::{diagnostics, junit, stacktrace};

/// How long to wait for a `--debug-jvm` JVM to start listening.
const DEBUG_PORT_TIMEOUT: Duration = Duration::from_secs(180);
/// After a debug session ends, how long a still-running build-tool child may
/// linger before it is killed.
const POST_SESSION_GRACE: Duration = Duration::from_secs(15);
/// Cap on process events handled per tick so a chatty program cannot starve
/// input handling.
const EVENTS_PER_TICK: usize = 2_000;
/// Tail of build/test output kept for diagnostic parsing.
const LOG_CAP: usize = 1 << 20;

/// Where a launch request gets its plan from.
#[derive(Debug, Clone)]
pub enum LaunchSource {
    /// Whatever is runnable at a position in a file.
    Cursor {
        file: PathBuf,
        language_id: String,
        /// LSP position (0-based line, UTF-16 character).
        line: u32,
        character: u32,
        /// `auto`, `main` or `test`.
        target: String,
        project_root: PathBuf,
    },
    /// A configuration from `.ovim/debug.toml` or `hyperion.runConfigurations`.
    Config {
        config: Box<DebugRunConfig>,
        project_root: PathBuf,
    },
    /// Not a launch at all: gather configurations for the picker.
    ConfigLookup { project_root: PathBuf },
}

/// Something that can be started and re-started.
#[derive(Debug, Clone)]
pub struct LaunchRequest {
    pub mode: LaunchMode,
    pub source: LaunchSource,
    /// Debug adapter to use instead of the language's configured one
    /// (`:debug start <cmd> [args...]`).
    pub adapter: Option<(String, Vec<String>)>,
}

impl LaunchRequest {
    fn project_root(&self) -> &Path {
        match &self.source {
            LaunchSource::Cursor { project_root, .. }
            | LaunchSource::Config { project_root, .. }
            | LaunchSource::ConfigLookup { project_root } => project_root,
        }
    }
}

/// Configurations waiting for the user to choose one in the picker.
pub(crate) struct PendingPick {
    mode: LaunchMode,
    configs: Vec<DebugRunConfig>,
    project_root: PathBuf,
    adapter: Option<(String, Vec<String>)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Resolving,
    Building,
    Running,
    AwaitingDebugPort { deadline: Instant },
    StartingDebugger,
    Debugging { session_ended: Option<Instant> },
}

struct LaunchJob {
    run_id: u64,
    request: LaunchRequest,
    plan: Option<LaunchPlan>,
    stage: Stage,
    resolve: Option<(oneshot::Receiver<ResolveResult>, JoinHandle<()>)>,
    proc: Option<ProcessHandle>,
    proc_exit: Option<ExitInfo>,
    /// Tail of process output in arrival order (for summaries and tails).
    log: String,
    /// The same output per pipe. stdout and stderr are read concurrently, so
    /// arrival order can split a multi-line diagnostic (javac's snippet and
    /// caret lines) around an unrelated line from the other pipe; diagnostics
    /// are therefore parsed from each pipe on its own.
    log_stdout: String,
    log_stderr: String,
    started_wall: SystemTime,
    stopping: bool,
    /// The debug session ended (or was stopped) while a child still ran.
    session_end: Option<crate::dap::SessionEnd>,
}

fn append_capped(log: &mut String, text: &str) {
    log.push_str(text);
    log.push('\n');
    if log.len() > LOG_CAP {
        let mut cut = log.len() - LOG_CAP / 2;
        while !log.is_char_boundary(cut) {
            cut += 1;
        }
        log.drain(..cut);
    }
}

impl LaunchJob {
    fn append_log(&mut self, stream: StreamKind, text: &str) {
        append_capped(&mut self.log, text);
        match stream {
            StreamKind::Stdout => append_capped(&mut self.log_stdout, text),
            StreamKind::Stderr => append_capped(&mut self.log_stderr, text),
        }
    }

    fn clear_log(&mut self) {
        self.log.clear();
        self.log_stdout.clear();
        self.log_stderr.clear();
    }

    /// Compiler diagnostics from both pipes, de-duplicated, errors first
    /// within each pipe's own order.
    fn diagnostics(&self, cwd: Option<&Path>) -> Vec<crate::editor::QuickfixEntry> {
        let mut entries = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for log in [&self.log_stderr, &self.log_stdout] {
            for entry in crate::commands::parse_compiler_output_in(log, cwd) {
                let key = (
                    entry.filename.clone(),
                    entry.lnum,
                    entry.col,
                    entry.entry_type as u8,
                    entry.text.clone(),
                );
                if seen.insert(key) {
                    entries.push(entry);
                }
            }
        }
        entries
    }
}

/// All launch-related editor state.
pub(crate) struct LaunchState {
    pub(crate) console: crate::launch::RunConsoleState,
    job: Option<LaunchJob>,
    last_request: Option<LaunchRequest>,
    /// A request waiting for the current job to stop (rerun replaces).
    queued: Option<LaunchRequest>,
    pending_pick: Option<PendingPick>,
    debug_port_timeout: Duration,
    pub(crate) code_lens: super::code_lens::CodeLensState,
}

impl Default for LaunchState {
    fn default() -> Self {
        Self {
            console: Default::default(),
            job: None,
            last_request: None,
            queued: None,
            pending_pick: None,
            debug_port_timeout: DEBUG_PORT_TIMEOUT,
            code_lens: Default::default(),
        }
    }
}

fn stream_kind(mapping: (LineKind, LineKind), stream: StreamKind) -> LineKind {
    match stream {
        StreamKind::Stdout => mapping.0,
        StreamKind::Stderr => mapping.1,
    }
}

impl Editor {
    // ------------------------------------------------------------------
    // Queries
    // ------------------------------------------------------------------

    /// The run console (panel state and all retained runs).
    pub fn run_console(&self) -> &crate::launch::RunConsoleState {
        &self.launch.console
    }

    pub fn run_console_mut(&mut self) -> &mut crate::launch::RunConsoleState {
        &mut self.launch.console
    }

    /// Whether a launch (resolve, build, run, or debug session) is in flight.
    pub fn is_launch_active(&self) -> bool {
        self.launch.job.is_some()
    }

    /// Overrides how long a `--debug-jvm` launch waits for the JVM to listen
    /// (default 3 minutes). Exists for tests and slow build machines.
    #[doc(hidden)]
    pub fn set_debug_port_timeout(&mut self, timeout: Duration) {
        self.launch.debug_port_timeout = timeout;
    }

    /// The request `:RunLast` would repeat.
    pub fn last_launch_request(&self) -> Option<&LaunchRequest> {
        self.launch.last_request.as_ref()
    }

    // ------------------------------------------------------------------
    // Entry points (called from keys and commands; never block)
    // ------------------------------------------------------------------

    /// Run or debug whatever is at the cursor (`target: auto`).
    pub fn launch_at_cursor(&mut self, mode: LaunchMode) {
        self.launch_at_cursor_with(mode, None);
    }

    /// Like [`launch_at_cursor`](Self::launch_at_cursor) with a custom debug
    /// adapter command (`:debug start <cmd> [args]`).
    pub fn launch_at_cursor_with(
        &mut self,
        mode: LaunchMode,
        adapter: Option<(String, Vec<String>)>,
    ) {
        match self.cursor_launch_source("auto") {
            Ok(source) => self.begin_request(LaunchRequest {
                mode,
                source,
                adapter,
            }),
            Err(message) => self.set_status_message(message),
        }
    }

    /// Run or debug whatever is at a position (0-based line, UTF-16
    /// character) of the current file, e.g. a code lens.
    pub fn launch_at_position(&mut self, mode: LaunchMode, line: usize, character: u32) {
        match self.cursor_launch_source("auto") {
            Ok(LaunchSource::Cursor {
                file,
                language_id,
                target,
                project_root,
                ..
            }) => self.begin_request(LaunchRequest {
                mode,
                source: LaunchSource::Cursor {
                    file,
                    language_id,
                    line: line as u32,
                    character,
                    target,
                    project_root,
                },
                adapter: None,
            }),
            Ok(_) => {}
            Err(message) => self.set_status_message(message),
        }
    }

    /// Run or debug a configuration chosen by the user.
    pub fn launch_config(
        &mut self,
        mode: LaunchMode,
        config: DebugRunConfig,
        project_root: PathBuf,
    ) {
        self.begin_request(LaunchRequest {
            mode,
            source: LaunchSource::Config {
                config: Box::new(config),
                project_root,
            },
            adapter: None,
        });
    }

    /// Repeat the last launch, replacing whatever is running now.
    pub fn launch_last(&mut self) {
        match self.launch.last_request.clone() {
            Some(request) => self.begin_request(request),
            None => self.set_status_message(
                "Nothing to rerun yet. Run the code at the cursor with <Space>rr, or debug it with <Space>rd",
            ),
        }
    }

    /// Open the picker of available configurations
    /// (`.ovim/debug.toml` + `hyperion.runConfigurations`).
    pub fn launch_pick_config(&mut self, mode: LaunchMode) {
        let project_root = self.launch_project_root(None);
        let mut loaded = crate::debug_config::load_debug_configs_reporting(&project_root);
        for problem in std::mem::take(&mut loaded.problems) {
            self.set_status_message(problem);
        }
        let file = self.buffer().file_path().map(PathBuf::from);
        let language_id = file
            .as_ref()
            .and_then(|f| self.language_id_for_path(&f.to_string_lossy()));
        let manager = self.lsp_manager();
        let (Some(manager), Some(file), Some(language_id), Ok(_)) = (
            manager,
            file,
            language_id,
            tokio::runtime::Handle::try_current(),
        ) else {
            self.offer_configs(
                mode,
                loaded.configs,
                project_root,
                None,
                "no language server",
            );
            return;
        };
        // The LSP half is fetched off-tick, then merged with the TOML half.
        let run_id = self.launch.console.start_run(
            "Run configurations".to_string(),
            mode,
            project_root.clone(),
        );
        let (tx, rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            let result = ResolveResult {
                outcome: ResolveOutcome::Nothing,
                configurations: lsp::run_configurations(&manager, &language_id, &file).await,
            };
            let _ = tx.send(result);
        });
        let request = LaunchRequest {
            mode,
            source: LaunchSource::ConfigLookup {
                project_root: project_root.clone(),
            },
            adapter: None,
        };
        // Stash the TOML configs in the pick state so the merge below has them.
        self.launch.pending_pick = Some(PendingPick {
            mode,
            configs: loaded.configs,
            project_root,
            adapter: None,
        });
        self.launch.job = Some(LaunchJob {
            run_id,
            request,
            plan: None,
            stage: Stage::Resolving,
            resolve: Some((rx, task)),
            proc: None,
            proc_exit: None,
            log: String::new(),
            log_stdout: String::new(),
            log_stderr: String::new(),
            started_wall: SystemTime::now(),
            stopping: false,
            session_end: None,
        });
        self.log_console(run_id, LineKind::System, "Looking up run configurations...");
    }

    /// Stop everything: resolve task, build, running program, debug session.
    /// Returns false when there was nothing to stop.
    pub fn launch_stop(&mut self) -> bool {
        self.launch.queued = None;
        let mut stopped = self.stop_current_job();
        if self.dap_manager.is_active() && self.launch.job.is_none() {
            // A debug session started by hand (not through a launch job).
            self.dap_manager.pending_action = Some(PendingDebugAction::Stop);
            stopped = true;
        }
        if !stopped {
            self.set_status_message("Nothing is running");
        }
        stopped
    }

    pub fn toggle_run_console(&mut self) {
        let console = &mut self.launch.console;
        console.open = !console.open;
        if !console.open && self.mode == crate::mode::Mode::RunConsole {
            self.mode = crate::mode::Mode::Normal;
        }
        if self.launch.console.open && self.launch.console.runs.is_empty() {
            self.set_status_message(
                "Run console. <Space>rr runs the code at the cursor, <Space>rd debugs it, <Space>rl reruns the last one",
            );
        }
        self.mark_dirty();
    }

    /// Give the run console keyboard focus (scroll, jump, rerun, stop).
    pub fn focus_run_console(&mut self) {
        self.launch.console.open = true;
        let last = self
            .launch
            .console
            .viewed()
            .map(|r| r.lines.len().saturating_sub(1))
            .unwrap_or(0);
        self.launch.console.cursor = last;
        self.launch.console.scroll = 0;
        self.mode = crate::mode::Mode::RunConsole;
        self.mark_dirty();
    }

    pub fn clear_run_console(&mut self) {
        self.launch.console.clear_finished();
        self.mark_dirty();
    }

    /// Jump to the source location on a console line. Returns whether a
    /// location was opened.
    pub fn run_console_jump(&mut self, line_index: usize) -> bool {
        let Some(run) = self.launch.console.viewed() else {
            return false;
        };
        let Some(line) = run.lines.get(line_index) else {
            return false;
        };
        let Some(location) = line.location.clone() else {
            self.set_status_message("No source location on this line");
            return false;
        };
        let (cwd, roots) = (run.cwd.clone(), run.source_roots.clone());
        match stacktrace::resolve_location(&location, &cwd, &roots) {
            Some((path, line, col)) => {
                self.open_location(&path, line, col);
                true
            }
            None => {
                let what = match &location {
                    stacktrace::ConsoleLocation::Frame { class, file, .. } => {
                        format!("{class} ({file})")
                    }
                    stacktrace::ConsoleLocation::File { path, .. } => path.display().to_string(),
                };
                self.set_status_message(format!("Source not found for {what}"));
                false
            }
        }
    }

    /// Opens `path` at a 1-based line/column and leaves console focus.
    pub(crate) fn open_location(&mut self, path: &Path, line: usize, col: usize) {
        if let Err(e) = self.open_file(path) {
            self.set_status_message(format!("Failed to open {}: {e}", path.display()));
            return;
        }
        if self.mode == crate::mode::Mode::RunConsole {
            self.mode = crate::mode::Mode::Normal;
        }
        let line0 = line.saturating_sub(1);
        self.buffer_mut()
            .cursor_mut()
            .set_position(line0, crate::unicode::GraphemeCol(col.saturating_sub(1)));
        self.buffer_mut().validate_cursor_position();
        self.center_cursor_in_viewport();
        self.mark_dirty();
    }

    /// A configuration was chosen in the picker.
    pub(crate) fn select_launch_config(&mut self, index: usize) {
        let Some(pick) = self.launch.pending_pick.take() else {
            self.set_status_message("No configuration list is open");
            return;
        };
        let Some(config) = pick.configs.get(index).cloned() else {
            self.set_status_message("Invalid configuration index");
            return;
        };
        self.begin_request(LaunchRequest {
            mode: pick.mode,
            source: LaunchSource::Config {
                config: Box::new(config),
                project_root: pick.project_root,
            },
            adapter: pick.adapter,
        });
    }

    // ------------------------------------------------------------------
    // Request setup
    // ------------------------------------------------------------------

    /// Workspace root for `file` (or the current buffer): the language
    /// server's root when one owns the file, else the nearest build marker,
    /// else the file's directory, else the process working directory.
    fn launch_project_root(&self, file: Option<&Path>) -> PathBuf {
        let current = self.buffer().file_path().map(PathBuf::from);
        let Some(file) = file.map(Path::to_path_buf).or(current) else {
            return std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        };
        if let (Some(manager), Some(language_id)) = (
            self.lsp_manager(),
            self.language_id_for_path(&file.to_string_lossy()),
        ) {
            for id in manager.servers_for_document(&language_id, &file) {
                if let Some(root) = manager.server_root(&id) {
                    return root;
                }
            }
        }
        const MARKERS: &[&str] = &[
            "settings.gradle.kts",
            "settings.gradle",
            "pom.xml",
            "build.gradle.kts",
            "build.gradle",
            ".ovim",
            ".git",
        ];
        let markers: Vec<String> = MARKERS.iter().map(|m| m.to_string()).collect();
        let root = crate::language_config::find_project_root(&file, &markers);
        if root.as_os_str().is_empty() {
            file.parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."))
        } else {
            root
        }
    }

    fn cursor_launch_source(&self, target: &str) -> Result<LaunchSource, String> {
        let Some(file) = self.buffer().file_path().map(PathBuf::from) else {
            return Err("Save the buffer to a file before running it".to_string());
        };
        let language_id = self
            .language_id_for_path(&file.to_string_lossy())
            .ok_or_else(|| format!("Don't know how to run {}", file.display()))?;
        let cursor = self.buffer().cursor();
        let character = self.col_to_utf16(cursor.line(), cursor.col().0);
        Ok(LaunchSource::Cursor {
            project_root: self.launch_project_root(Some(&file)),
            file,
            language_id,
            line: cursor.line() as u32,
            character,
            target: target.to_string(),
        })
    }

    /// Starts a request, replacing whatever is running.
    fn begin_request(&mut self, request: LaunchRequest) {
        if tokio::runtime::Handle::try_current().is_err() {
            self.set_status_message("Cannot start a run: no async runtime available");
            return;
        }
        if self.launch.job.is_some() || self.dap_manager.is_active() {
            // Rerun / new run replaces the current one: stop it, start when
            // it is gone.
            self.launch.queued = Some(request);
            if !self.stop_current_job() && self.dap_manager.is_active() {
                self.dap_manager.pending_action = Some(PendingDebugAction::Stop);
            }
            self.set_status_message("Stopping the current run...");
            return;
        }
        self.launch.last_request = Some(request.clone());
        self.save_before_launch();

        let mode = request.mode;
        let root = request.project_root().to_path_buf();
        let title = match &request.source {
            LaunchSource::Cursor { file, .. } => file
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "file".to_string()),
            LaunchSource::Config { config, .. } => config.name.clone(),
            LaunchSource::ConfigLookup { .. } => "Run configurations".to_string(),
        };
        let run_id = self
            .launch
            .console
            .start_run(title.clone(), mode, root.clone());
        if let Some(run) = self.launch.console.run_mut(run_id) {
            run.source_roots = vec![root.clone()];
        }
        let mut job = LaunchJob {
            run_id,
            request: request.clone(),
            plan: None,
            stage: Stage::Resolving,
            resolve: None,
            proc: None,
            proc_exit: None,
            log: String::new(),
            log_stdout: String::new(),
            log_stderr: String::new(),
            started_wall: SystemTime::now(),
            stopping: false,
            session_end: None,
        };

        match request.source.clone() {
            LaunchSource::ConfigLookup { .. } => {}
            LaunchSource::Config {
                config,
                project_root,
            } => {
                let plan = plan::plan_from_config(&config, &project_root);
                self.begin_plan(&mut job, plan);
            }
            LaunchSource::Cursor {
                file,
                language_id,
                line,
                character,
                target,
                ..
            } => {
                self.log_console(
                    run_id,
                    LineKind::System,
                    format!(
                        "Resolving what to {} at the cursor...",
                        mode.verb().to_lowercase()
                    ),
                );
                match self.lsp_manager() {
                    Some(manager) => {
                        let (tx, rx) = oneshot::channel();
                        let task = tokio::spawn(async move {
                            let result = lsp::resolve_with_fallback(
                                manager,
                                language_id,
                                file,
                                line,
                                character,
                                target,
                            )
                            .await;
                            let _ = tx.send(result);
                        });
                        job.resolve = Some((rx, task));
                    }
                    None => {
                        let result = ResolveResult {
                            outcome: ResolveOutcome::NoServer(
                                "the language server is not running".to_string(),
                            ),
                            configurations: Vec::new(),
                        };
                        self.on_resolved(&mut job, result);
                    }
                }
            }
        }
        self.launch_put_back(job);
        self.mark_dirty();
    }

    /// Keeps a job that is still in flight; finished jobs were consumed by
    /// `finish_job` and must not be re-inserted.
    fn launch_put_back(&mut self, job: LaunchJob) {
        let finished = self
            .launch
            .console
            .run(job.run_id)
            .is_none_or(|r| !r.status.is_active());
        if !finished {
            self.launch.job = Some(job);
        } else {
            self.start_queued_launch();
        }
    }

    fn start_queued_launch(&mut self) {
        if self.launch.job.is_none() && !self.dap_manager.is_active() {
            if let Some(next) = self.launch.queued.take() {
                self.begin_request(next);
            }
        }
    }

    /// Saves modified buffers so the build sees the code on screen.
    fn save_before_launch(&mut self) {
        let (_, errors) = self.write_all_modified_buffers(false);
        if let Some(first) = errors.first() {
            self.set_status_message(format!(
                "Could not save all files before running ({first}); running what is on disk"
            ));
        }
    }

    fn log_console(&mut self, run_id: u64, kind: LineKind, text: impl Into<String>) {
        if let Some(run) = self.launch.console.run_mut(run_id) {
            run.push_line(kind, text);
        }
        self.mark_dirty();
    }

    // ------------------------------------------------------------------
    // Resolution
    // ------------------------------------------------------------------

    fn on_resolved(&mut self, job: &mut LaunchJob, result: ResolveResult) {
        let mode = job.request.mode;
        // The config picker (`launch_pick_config`) has its own tail.
        if matches!(&job.request.source, LaunchSource::ConfigLookup { .. }) {
            self.finish_config_lookup(job, result);
            return;
        }
        match result.outcome {
            ResolveOutcome::Plan(value) => match plan::plan_from_resolved(&value) {
                Ok(Some(plan)) => self.begin_plan(job, plan),
                Ok(None) => {
                    self.no_plan_here(job, "nothing runnable at the cursor", result.configurations)
                }
                Err(message) => self.fail_job(job, message),
            },
            ResolveOutcome::Nothing => self.no_plan_here(
                job,
                "no main method or test at the cursor",
                result.configurations,
            ),
            ResolveOutcome::Unsupported(reason)
            | ResolveOutcome::NoServer(reason)
            | ResolveOutcome::Failed(reason) => {
                let _ = mode;
                self.no_plan_here(job, &reason, result.configurations)
            }
        }
    }

    /// The server gave no launch information for the cursor: fall back to
    /// configurations, or explain what to do.
    fn no_plan_here(
        &mut self,
        job: &mut LaunchJob,
        reason: &str,
        lsp_configs: Vec<serde_json::Value>,
    ) {
        let mode = job.request.mode;
        let root = job.request.project_root().to_path_buf();
        let mut loaded = crate::debug_config::load_debug_configs_reporting(&root);
        for problem in std::mem::take(&mut loaded.problems) {
            self.log_console(job.run_id, LineKind::System, format!("warning: {problem}"));
        }
        let mut configs = loaded.configs;
        configs.extend(crate::debug_config::parse_lsp_run_configs(&lsp_configs));
        self.offer_configs_for_job(job, mode, configs, reason);
    }

    fn finish_config_lookup(&mut self, job: &mut LaunchJob, result: ResolveResult) {
        let mode = job.request.mode;
        let mut configs = self
            .launch
            .pending_pick
            .take()
            .map(|p| p.configs)
            .unwrap_or_default();
        configs.extend(crate::debug_config::parse_lsp_run_configs(
            &result.configurations,
        ));
        let root = job.request.project_root().to_path_buf();
        self.discard_run(job);
        self.offer_configs(mode, configs, root, None, "");
    }

    /// Chooses among configurations when a job could not resolve a plan.
    fn offer_configs_for_job(
        &mut self,
        job: &mut LaunchJob,
        mode: LaunchMode,
        configs: Vec<DebugRunConfig>,
        reason: &str,
    ) {
        let root = job.request.project_root().to_path_buf();
        let runnable: Vec<DebugRunConfig> = configs
            .into_iter()
            .filter(|c| {
                let plan = plan::plan_from_config(c, &root);
                plan.unsupported_reason(mode).is_none()
            })
            .collect();
        match runnable.len() {
            0 => {
                let what = match mode {
                    LaunchMode::Run => "run",
                    LaunchMode::Debug => "debug",
                };
                self.fail_job(
                    job,
                    format!(
                        "Nothing to {what} here: {reason}. Put the cursor in a class with a main method or in a test, \
                         or add a configuration to .ovim/debug.toml"
                    ),
                );
            }
            1 => {
                let plan = plan::plan_from_config(&runnable[0], &root);
                self.log_console(
                    job.run_id,
                    LineKind::System,
                    format!("({reason}; using configuration '{}')", runnable[0].name),
                );
                self.begin_plan(job, plan);
            }
            _ => {
                self.discard_run(job);
                let adapter = job.request.adapter.clone();
                self.offer_configs(mode, runnable, root, adapter, reason);
            }
        }
    }

    /// Shows the configuration picker (or explains why there is nothing to pick).
    fn offer_configs(
        &mut self,
        mode: LaunchMode,
        configs: Vec<DebugRunConfig>,
        project_root: PathBuf,
        adapter: Option<(String, Vec<String>)>,
        reason: &str,
    ) {
        let configs: Vec<DebugRunConfig> = configs
            .into_iter()
            .filter(|c| {
                plan::plan_from_config(c, &project_root)
                    .unsupported_reason(mode)
                    .is_none()
            })
            .collect();
        if configs.is_empty() {
            self.set_status_message(
                "No run configurations found. Add one to .ovim/debug.toml, or run the code at the cursor with <Space>rr",
            );
            return;
        }
        let names: Vec<String> = configs.iter().map(|c| c.name.clone()).collect();
        self.launch.pending_pick = Some(PendingPick {
            mode,
            configs,
            project_root: project_root.clone(),
            adapter,
        });
        let picker = crate::editor::picker::Picker::new_debug_config(project_root, names);
        self.set_picker(picker);
        self.set_mode(crate::mode::Mode::Picker);
        self.mark_picker_selection_changed();
        if !reason.is_empty() {
            self.set_status_message(format!("{reason}: pick a configuration"));
        }
    }

    /// Removes the console record of a job that turned into a picker.
    fn discard_run(&mut self, job: &LaunchJob) {
        self.launch.console.runs.retain(|r| r.id != job.run_id);
        self.launch.console.selected = None;
        if self.launch.console.runs.is_empty() {
            self.launch.console.open = false;
        }
        // Consumed: mark finished so `launch_put_back` drops it.
    }

    // ------------------------------------------------------------------
    // Plan execution
    // ------------------------------------------------------------------

    fn begin_plan(&mut self, job: &mut LaunchJob, plan: LaunchPlan) {
        let mode = job.request.mode;
        if let Some(reason) = plan.unsupported_reason(mode) {
            self.fail_job(job, reason);
            return;
        }
        if let Some(run) = self.launch.console.run_mut(job.run_id) {
            run.title = plan.name.clone();
            run.source_roots = plan.source_roots();
            run.cwd = plan.project_root.clone();
        }
        for warning in &plan.warnings {
            self.log_console(job.run_id, LineKind::System, format!("warning: {warning}"));
        }
        job.plan = Some(plan.clone());

        match (plan.kind, mode) {
            (PlanKind::Attach, _) => {
                let attach = plan.attach.clone().expect("attach plans carry a target");
                let request = DapLaunchRequest::Attach(serde_json::json!({
                    "host": attach.host,
                    "port": attach.port,
                    "projectRoot": attach.project_root,
                }));
                self.log_console(
                    job.run_id,
                    LineKind::System,
                    format!("Attaching to {}:{}", attach.host, attach.port),
                );
                self.begin_debugger(job, request);
            }
            (PlanKind::Main, _) if plan.build.is_some() => self.start_build(job),
            (PlanKind::Main, LaunchMode::Run) => self.start_run_process(job),
            (PlanKind::Main, LaunchMode::Debug) => {
                let launch = plan.launch.as_ref().expect("main plans carry launch");
                let request = DapLaunchRequest::Launch(launch.dap_arguments.clone());
                self.begin_debugger(job, request);
            }
            (PlanKind::Test | PlanKind::Task, LaunchMode::Run) => self.start_run_process(job),
            (PlanKind::Test | PlanKind::Task, LaunchMode::Debug) => self.start_debug_task(job),
        }
    }

    fn set_phase(&mut self, job: &LaunchJob, phase: RunPhase) {
        if let Some(run) = self.launch.console.run_mut(job.run_id) {
            run.status = RunStatus::Active(phase);
        }
        self.mark_dirty();
    }

    fn spawn_step(
        &mut self,
        job: &mut LaunchJob,
        spec: &CommandSpec,
        phase: RunPhase,
        stage: Stage,
    ) {
        self.log_console(
            job.run_id,
            LineKind::System,
            format!("$ {}", spec.display()),
        );
        if let Some(run) = self.launch.console.run_mut(job.run_id) {
            run.command = spec.display();
            run.cwd = spec.cwd.clone();
        }
        match ProcessHandle::spawn(spec) {
            Ok(proc) => {
                job.proc = Some(proc);
                job.proc_exit = None;
                job.clear_log();
                job.stage = stage;
                self.set_phase(job, phase);
            }
            Err(message) => self.fail_job(job, message),
        }
    }

    fn start_build(&mut self, job: &mut LaunchJob) {
        let Some(build) = job.plan.as_ref().and_then(|p| p.build.clone()) else {
            return;
        };
        self.spawn_step(job, &build, RunPhase::Building, Stage::Building);
    }

    fn start_run_process(&mut self, job: &mut LaunchJob) {
        let Some(plan) = job.plan.clone() else { return };
        let spec = match (plan.kind, &plan.launch, &plan.task) {
            (PlanKind::Main, Some(launch), _) => launch.run_command(),
            (_, _, Some(task)) => CommandSpec {
                argv: task.argv.clone(),
                cwd: task.cwd.clone(),
                env: Default::default(),
            },
            _ => return,
        };
        job.started_wall = SystemTime::now();
        self.spawn_step(job, &spec, RunPhase::Running, Stage::Running);
    }

    fn start_debug_task(&mut self, job: &mut LaunchJob) {
        let Some(task) = job.plan.as_ref().and_then(|p| p.task.clone()) else {
            return;
        };
        let Some(argv) = task.debug_argv.clone() else {
            return;
        };
        let spec = CommandSpec {
            argv,
            cwd: task.cwd.clone(),
            env: Default::default(),
        };
        job.started_wall = SystemTime::now();
        self.spawn_step(
            job,
            &spec,
            RunPhase::WaitingForDebugger,
            Stage::AwaitingDebugPort {
                deadline: Instant::now() + self.launch.debug_port_timeout,
            },
        );
    }

    /// Queues the debug adapter start for `request`.
    fn begin_debugger(&mut self, job: &mut LaunchJob, request: DapLaunchRequest) {
        let (command, args) = match self.debug_adapter_for(job) {
            Ok(found) => found,
            Err(message) => {
                self.fail_job(job, message);
                return;
            }
        };
        self.log_console(
            job.run_id,
            LineKind::System,
            format!("Starting debug adapter: {command} {}", args.join(" ")),
        );
        job.stage = Stage::StartingDebugger;
        self.set_phase(job, RunPhase::Debugging);
        self.dap_manager.pending_action = Some(PendingDebugAction::Start {
            command,
            args,
            launch: request,
        });
    }

    fn debug_adapter_for(&self, job: &LaunchJob) -> Result<(String, Vec<String>), String> {
        if let Some(custom) = &job.request.adapter {
            return Ok(custom.clone());
        }
        let language = job
            .plan
            .as_ref()
            .and_then(|p| p.language.clone())
            .or_else(|| match &job.request.source {
                LaunchSource::Cursor { language_id, .. } => Some(language_id.clone()),
                _ => None,
            })
            .unwrap_or_else(|| "java".to_string());
        let registry = crate::language_config::LanguageRegistry::try_get()
            .ok_or("language registry unavailable")?;
        let config = registry
            .get_by_id(&language)
            .and_then(|l| l.dap.as_ref())
            .ok_or_else(|| {
                format!("No debug adapter is configured for {language}. Use :debug start <command> [args...]")
            })?;
        match crate::language_config::find_dap_command(config) {
            Some(command) => Ok((command, config.args.clone())),
            None => Err(format!(
                "Debug adapter '{}' not found. {}",
                config.command,
                config
                    .install_hint
                    .as_deref()
                    .unwrap_or("Install it and make sure it is on PATH")
            )),
        }
    }

    // ------------------------------------------------------------------
    // Debug session hooks (called from the tick's async DAP handling)
    // ------------------------------------------------------------------

    /// The adapter accepted launch/attach and breakpoints are synced.
    pub fn launch_debug_started(&mut self) {
        if let Some(mut job) = self.launch.job.take() {
            if matches!(job.stage, Stage::StartingDebugger) {
                job.stage = Stage::Debugging {
                    session_ended: None,
                };
                self.log_console(job.run_id, LineKind::System, "Debugger attached");
                self.set_phase(&job, RunPhase::Debugging);
            }
            self.launch.job = Some(job);
        }
        self.clear_status_message();
    }

    /// Starting the adapter, launching, or attaching failed. The session is
    /// torn down completely (adapter killed, state reset).
    pub fn launch_debug_failed(&mut self, message: String) {
        self.dap_manager.pending_action = None;
        self.dap_manager.state.end_session_keep_output();
        self.dap_manager.launch_request = None;
        if let Some(mut job) = self.launch.job.take() {
            self.fail_job(&mut job, format!("Debug failed: {message}"));
        } else {
            self.set_status_message(format!("Debug failed: {message}"));
        }
    }

    // ------------------------------------------------------------------
    // Polling (called every tick)
    // ------------------------------------------------------------------

    /// Advances the launch state machine. Returns true when a redraw is needed.
    pub fn poll_launch(&mut self) -> bool {
        let mut changed = self.ingest_debug_output();
        let Some(mut job) = self.launch.job.take() else {
            changed |= self.acknowledge_orphan_session_end();
            self.start_queued_launch();
            return changed;
        };

        // -- resolving --
        if let Some((mut rx, task)) = job.resolve.take() {
            match rx.try_recv() {
                Ok(result) => {
                    self.on_resolved(&mut job, result);
                    changed = true;
                }
                Err(oneshot::error::TryRecvError::Empty) => {
                    job.resolve = Some((rx, task));
                }
                Err(oneshot::error::TryRecvError::Closed) => {
                    self.fail_job(
                        &mut job,
                        "The language server request was cancelled".to_string(),
                    );
                    changed = true;
                }
            }
        }

        // -- process output --
        if job.proc.is_some() {
            changed |= self.drain_process(&mut job);
        }

        // -- debug session end / timeouts --
        changed |= self.poll_debug_state(&mut job);

        self.launch_put_back(job);
        if self.launch.console.any_active() {
            // Keeps the elapsed timer live.
            changed = true;
        }
        changed
    }

    /// A session ended with no job to attribute it to (manual adapter start).
    fn acknowledge_orphan_session_end(&mut self) -> bool {
        self.dap_manager.take_session_end().is_some()
    }

    fn drain_process(&mut self, job: &mut LaunchJob) -> bool {
        let mapping = match job.stage {
            Stage::Building => (LineKind::Build, LineKind::Stderr),
            _ => (LineKind::Stdout, LineKind::Stderr),
        };
        let mut changed = false;
        let mut exit: Option<ExitInfo> = None;
        for _ in 0..EVENTS_PER_TICK {
            let Some(proc) = job.proc.as_mut() else { break };
            match proc.try_recv() {
                Some(ProcEvent::Line { stream, text }) => {
                    changed = true;
                    job.append_log(stream, &text);
                    if let Some(run) = self.launch.console.run_mut(job.run_id) {
                        run.push_line(stream_kind(mapping, stream), text.clone());
                    }
                    if let Stage::AwaitingDebugPort { .. } = job.stage {
                        if let Some(port) = plan::parse_listening_port(&text) {
                            self.attach_to_listening_jvm(job, port);
                        }
                    }
                }
                Some(ProcEvent::Exit(info)) => {
                    exit = Some(info);
                    break;
                }
                None => break,
            }
        }
        if let Some(info) = exit {
            job.proc = None;
            job.proc_exit = Some(info);
            self.on_process_exit(job, info);
            changed = true;
        }
        changed
    }

    fn attach_to_listening_jvm(&mut self, job: &mut LaunchJob, port: u16) {
        let root = job
            .plan
            .as_ref()
            .map(|p| p.project_root.clone())
            .unwrap_or_default();
        self.log_console(
            job.run_id,
            LineKind::System,
            format!("JVM is listening on port {port}; attaching debugger"),
        );
        let request = DapLaunchRequest::Attach(serde_json::json!({
            "host": "127.0.0.1",
            "port": port,
            "projectRoot": root,
        }));
        self.begin_debugger(job, request);
    }

    fn on_process_exit(&mut self, job: &mut LaunchJob, info: ExitInfo) {
        let code = info.code;
        let code_text = code
            .map(|c| c.to_string())
            .unwrap_or_else(|| "signal".to_string());
        match job.stage {
            Stage::Building => {
                if info.killed {
                    self.log_console(job.run_id, LineKind::System, "Build stopped");
                    self.finish_job(job, RunOutcome::Stopped, code);
                } else if code == Some(0) {
                    self.build_succeeded(job);
                } else {
                    self.log_console(
                        job.run_id,
                        LineKind::System,
                        format!("Build failed (exit {code_text}); launch aborted"),
                    );
                    let summary = self.report_build_problems(job, true);
                    let message = match summary {
                        Some(s) => format!("Build failed: {s}. Launch aborted"),
                        None => format!("Build failed (exit {code_text}). Launch aborted"),
                    };
                    self.set_status_message(message);
                    self.finish_job(job, RunOutcome::BuildFailed, code);
                }
            }
            Stage::Running => {
                if info.killed {
                    self.log_console(job.run_id, LineKind::System, "Stopped");
                    self.finish_job(job, RunOutcome::Stopped, code);
                    return;
                }
                let ok = code == Some(0);
                self.log_console(
                    job.run_id,
                    LineKind::System,
                    format!("Process finished with exit code {code_text}"),
                );
                self.finish_test_reports(job);
                if !ok {
                    let summary = self.report_build_problems(job, false);
                    if let Some(s) = summary {
                        self.set_status_message(s);
                    }
                }
                self.finish_job(
                    job,
                    if ok {
                        RunOutcome::Succeeded
                    } else {
                        RunOutcome::Failed
                    },
                    code,
                );
            }
            Stage::AwaitingDebugPort { .. } => {
                if info.killed {
                    self.finish_job(job, RunOutcome::Stopped, code);
                    return;
                }
                let summary = self.report_build_problems(job, true);
                let tail = last_lines(&job.log, 3);
                let message = format!(
                    "The JVM never started listening for a debugger (exit {code_text}){}{}",
                    summary.map(|s| format!(": {s}")).unwrap_or_default(),
                    if tail.is_empty() {
                        String::new()
                    } else {
                        format!(". Last output: {tail}")
                    }
                );
                self.fail_job(job, message);
            }
            Stage::StartingDebugger | Stage::Debugging { .. } => {
                // The build-tool child of a test-debug session ended.
                self.log_console(
                    job.run_id,
                    LineKind::System,
                    format!("Process finished with exit code {code_text}"),
                );
                self.finish_test_reports(job);
            }
            Stage::Resolving => {}
        }
    }

    fn build_succeeded(&mut self, job: &mut LaunchJob) {
        self.log_console(job.run_id, LineKind::System, "Build succeeded");
        // Warnings still go to quickfix (silently, no jump).
        let cwd = job
            .plan
            .as_ref()
            .and_then(|p| p.build.as_ref())
            .map(|b| b.cwd.clone());
        let entries = job.diagnostics(cwd.as_deref());
        if entries.is_empty() {
            self.close_quickfix_window();
            self.set_quickfix_list(Vec::new(), String::new());
        } else {
            self.set_quickfix_list(entries, "build".to_string());
        }
        let Some(plan) = job.plan.clone() else { return };
        job.clear_log();
        match (plan.kind, job.request.mode) {
            (_, LaunchMode::Run) => self.start_run_process(job),
            (_, LaunchMode::Debug) => {
                let launch = plan.launch.as_ref().expect("built plans are main plans");
                let request = DapLaunchRequest::Launch(launch.dap_arguments.clone());
                self.begin_debugger(job, request);
            }
        }
    }

    /// Parses the job's build/test output into the quickfix list. With
    /// `jump`, opens the first error. Returns a one-line summary of the failure.
    fn report_build_problems(&mut self, job: &LaunchJob, jump: bool) -> Option<String> {
        let cwd = job.plan.as_ref().map(|p| {
            p.build
                .as_ref()
                .map(|b| b.cwd.clone())
                .or_else(|| p.task.as_ref().map(|t| t.cwd.clone()))
                .unwrap_or_else(|| p.project_root.clone())
        });
        let entries = job.diagnostics(cwd.as_deref());
        let summary = diagnostics::summarize_build_failure(&job.log, &entries);
        if entries.is_empty() {
            return summary;
        }
        let first_error = entries
            .iter()
            .position(|e| e.entry_type == crate::editor::QuickfixEntryType::Error)
            .unwrap_or(0);
        let title = job
            .plan
            .as_ref()
            .and_then(|p| p.build.as_ref())
            .map(|b| b.display())
            .unwrap_or_else(|| "build".to_string());
        self.set_quickfix_list(entries, title);
        self.ui_panels.quickfix_list.set_selected(first_error);
        if jump {
            self.jump_to_quickfix_entry();
        }
        self.open_quickfix_window();
        summary
    }

    /// After a test task, read JUnit XML from the plan's reports directory
    /// and surface the outcome in the console and the quickfix list.
    fn finish_test_reports(&mut self, job: &mut LaunchJob) {
        let Some(dir) = job
            .plan
            .as_ref()
            .and_then(|p| p.task.as_ref())
            .and_then(|t| t.reports_dir.clone())
        else {
            return;
        };
        let cases = junit::read_reports_dir(&dir, job.started_wall - Duration::from_secs(2));
        let Some(summary) = junit::summary_text(&cases) else {
            return;
        };
        self.log_console(job.run_id, LineKind::System, format!("Tests: {summary}"));
        let mut entries = Vec::new();
        for case in cases.iter().filter(|c| {
            matches!(
                c.status,
                junit::CaseStatus::Failed | junit::CaseStatus::Errored
            )
        }) {
            let label = format!("{}.{}", case.class_name, case.name);
            let message = case
                .message
                .clone()
                .unwrap_or_else(|| "test failed".to_string());
            self.log_console(
                job.run_id,
                LineKind::System,
                format!("FAILED {label}: {message}"),
            );
            // The first frame in a project file is where to go.
            let roots = job
                .plan
                .as_ref()
                .map(|p| p.source_roots())
                .unwrap_or_default();
            let location = case
                .details
                .as_deref()
                .into_iter()
                .flat_map(str::lines)
                .filter_map(stacktrace::parse_console_location)
                .find_map(|loc| stacktrace::resolve_location(&loc, &dir, &roots));
            if let Some((path, line, col)) = location {
                entries.push(crate::editor::QuickfixEntry::error(
                    Some(path),
                    line,
                    col,
                    format!("{label}: {message}"),
                ));
            }
        }
        if !entries.is_empty() {
            self.set_quickfix_list(entries, "test failures".to_string());
        }
        self.set_status_message(format!("Tests: {summary}"));
    }

    fn poll_debug_state(&mut self, job: &mut LaunchJob) -> bool {
        let mut changed = false;
        if let Stage::AwaitingDebugPort { deadline } = job.stage {
            if Instant::now() >= deadline {
                if let Some(proc) = job.proc.as_mut() {
                    proc.kill();
                }
                let tail = last_lines(&job.log, 3);
                let message = format!(
                    "Timed out after {} waiting for the JVM to listen for a debugger. Last output: {}",
                    crate::editor::format_duration(self.launch.debug_port_timeout),
                    if tail.is_empty() { "(none)".to_string() } else { tail }
                );
                self.fail_job(job, message);
                return true;
            }
        }
        if !matches!(job.stage, Stage::StartingDebugger | Stage::Debugging { .. }) {
            return changed;
        }
        if let Some(end) = self.dap_manager.take_session_end() {
            job.session_end = Some(end);
            job.stage = Stage::Debugging {
                session_ended: Some(Instant::now()),
            };
            changed = true;
        }
        if let Stage::Debugging {
            session_ended: Some(at),
        } = job.stage
        {
            let child_running = job.proc.is_some();
            if child_running && at.elapsed() > POST_SESSION_GRACE {
                if let Some(proc) = job.proc.as_mut() {
                    proc.kill();
                }
            }
            if !child_running {
                let end = job
                    .session_end
                    .unwrap_or(crate::dap::SessionEnd { exit_code: None });
                let outcome = if job.stopping {
                    RunOutcome::Stopped
                } else {
                    match end.exit_code {
                        Some(0) => RunOutcome::Succeeded,
                        Some(_) => RunOutcome::Failed,
                        None => RunOutcome::Ended,
                    }
                };
                let text = match (job.stopping, end.exit_code) {
                    (true, _) => "Debug session stopped".to_string(),
                    (false, Some(code)) => format!("Debug session ended (exit code {code})"),
                    (false, None) => "Debug session ended".to_string(),
                };
                self.log_console(job.run_id, LineKind::System, text);
                let exit = end.exit_code.or(job.proc_exit.and_then(|p| p.code));
                self.finish_job(job, outcome, exit);
                changed = true;
            }
        }
        changed
    }

    /// Debuggee/adapter output goes into the run console.
    fn ingest_debug_output(&mut self) -> bool {
        let output = self.dap_manager.take_console_output();
        if output.is_empty() {
            return false;
        }
        let run_id = match self.launch.job.as_ref() {
            Some(job) => job.run_id,
            None => {
                // A session started by hand: give it a record of its own.
                match self
                    .launch
                    .console
                    .runs
                    .last()
                    .filter(|r| r.status.is_active())
                {
                    Some(run) => run.id,
                    None => {
                        let cwd = std::env::current_dir().unwrap_or_default();
                        let id = self.launch.console.start_run(
                            "Debug session".to_string(),
                            LaunchMode::Debug,
                            cwd.clone(),
                        );
                        if let Some(run) = self.launch.console.run_mut(id) {
                            run.source_roots = vec![cwd];
                            run.status = RunStatus::Active(RunPhase::Debugging);
                        }
                        id
                    }
                }
            }
        };
        if let Some(run) = self.launch.console.run_mut(run_id) {
            for (category, text) in output {
                let kind = match category.as_str() {
                    "stdout" => LineKind::Stdout,
                    "stderr" => LineKind::Stderr,
                    _ => LineKind::Debugger,
                };
                run.push_chunk(kind, &text);
            }
        }
        true
    }

    // ------------------------------------------------------------------
    // Finishing and stopping
    // ------------------------------------------------------------------

    fn fail_job(&mut self, job: &mut LaunchJob, message: String) {
        if let Some(proc) = job.proc.as_mut() {
            proc.kill();
        }
        if let Some((_, task)) = job.resolve.take() {
            task.abort();
        }
        if matches!(job.stage, Stage::StartingDebugger | Stage::Debugging { .. })
            || self.dap_manager.is_active()
        {
            self.dap_manager.pending_action = Some(PendingDebugAction::Stop);
        }
        self.log_console(job.run_id, LineKind::System, format!("✗ {message}"));
        self.set_status_message(message.clone());
        self.finish_job(job, RunOutcome::Error(message), None);
    }

    fn finish_job(&mut self, job: &mut LaunchJob, outcome: RunOutcome, code: Option<i32>) {
        let status = self.launch.console.run_mut(job.run_id).map(|run| {
            run.finish(outcome.clone(), code);
            format!("{}: {}", run.title, run.status_text())
        });
        if let Some(status) = status {
            match &outcome {
                RunOutcome::Error(_) | RunOutcome::BuildFailed => {}
                _ => self.set_status_message(status),
            }
        }
        self.dap_manager.state.panels_visible = false;
        self.mark_dirty();
    }

    /// Asks the current job to stop. Returns true when there was a job.
    fn stop_current_job(&mut self) -> bool {
        let Some(mut job) = self.launch.job.take() else {
            return false;
        };
        job.stopping = true;
        let mut done = false;
        match job.stage {
            Stage::Resolving => {
                if let Some((_, task)) = job.resolve.take() {
                    task.abort();
                }
                self.log_console(job.run_id, LineKind::System, "Stopped");
                self.finish_job(&mut job, RunOutcome::Stopped, None);
                done = true;
            }
            Stage::Building | Stage::Running | Stage::AwaitingDebugPort { .. } => {
                if let Some(proc) = job.proc.as_mut() {
                    proc.kill();
                }
            }
            Stage::StartingDebugger | Stage::Debugging { .. } => {
                if let Some(proc) = job.proc.as_mut() {
                    proc.kill();
                }
                self.dap_manager.pending_action = Some(PendingDebugAction::Stop);
            }
        }
        if !done {
            self.launch.job = Some(job);
        }
        true
    }
}

fn last_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join(" | ")
}
