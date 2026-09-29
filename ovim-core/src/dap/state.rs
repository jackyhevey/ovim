//! Debug state tracking.
//!
//! Holds all debug-related state: breakpoints, stack frames, variables, output.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::types::{DapBreakpoint, DapScope, DapStackFrame, DapThread, DapVariable};

/// Per-line breakpoint state.
#[derive(Debug, Clone)]
pub struct BreakpointState {
    /// 1-based line number.
    pub line: u64,
    /// Whether the debug adapter confirmed this breakpoint.
    pub verified: bool,
    /// DAP-assigned breakpoint ID.
    pub id: Option<u64>,
    /// Condition expression for conditional breakpoints (None = unconditional).
    pub condition: Option<String>,
    /// Logpoint: print this message (with `{expression}` interpolation)
    /// instead of stopping.
    pub log_message: Option<String>,
    /// Hit-count condition (`5`, `>3`, `%2`): stop only when it holds.
    pub hit_condition: Option<String>,
    /// Disabled breakpoints stay in the list (and the gutter, hollow) but are
    /// not sent to the adapter.
    pub enabled: bool,
}

/// A watch expression, re-evaluated at every stop.
#[derive(Debug, Clone)]
pub struct Watch {
    pub expression: String,
    /// `Ok(value)` or `Err(message)` from the last evaluation while stopped.
    pub result: Option<Result<String, String>>,
    pub type_: Option<String>,
    /// Non-zero when the value has children that can be expanded.
    pub variables_reference: u64,
}

/// An exception category the adapter can break on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExceptionFilter {
    pub id: String,
    pub label: String,
    pub enabled: bool,
}

/// Cursor and scroll state of the focusable debug panel.
#[derive(Debug, Clone, Default)]
pub struct PanelUi {
    /// Highlighted row (index into [`panel::rows`](super::panel::rows)).
    pub cursor: usize,
    /// First visible row (written by the renderer, which knows the height).
    pub scroll: std::cell::Cell<usize>,
    /// Rows that fit on screen; set by the renderer for paging.
    pub view_height: std::cell::Cell<usize>,
    /// Columns added to (or taken from) the default panel width.
    pub width_delta: i16,
}

/// All debug state for the editor.
pub struct DebugState {
    /// Whether a debug session is active.
    pub session_active: bool,
    /// Whether the debuggee is currently running (not stopped).
    pub is_running: bool,

    // ---- Stop state ----
    /// Thread that is currently stopped (if any).
    pub stopped_thread: Option<u64>,
    /// The thread the adapter reported the stop on (`stopped_thread` is the
    /// one being inspected, which the user can change).
    pub event_thread: Option<u64>,
    /// Threads of the debuggee as of the last stop.
    pub threads: Vec<DapThread>,
    /// Reason for the stop (e.g., "breakpoint", "step", "exception").
    pub stop_reason: Option<String>,
    /// What was thrown (`Type: message`) when the stop reason is "exception".
    pub exception: Option<String>,

    // ---- Breakpoints ----
    /// Breakpoints per file path.
    pub breakpoints: HashMap<PathBuf, Vec<BreakpointState>>,

    // ---- Stack trace ----
    /// Stack frames from the last stop.
    pub stack_frames: Vec<DapStackFrame>,
    /// Currently selected frame index.
    pub selected_frame: usize,

    // ---- Variables ----
    /// Scopes for the selected frame.
    pub scopes: Vec<DapScope>,
    /// Variables by reference ID.
    pub variables: HashMap<u64, Vec<DapVariable>>,
    /// Expanded variable references (for tree view).
    pub expanded_refs: HashSet<u64>,

    // ---- Output ----
    /// Debuggee output lines.
    pub output_lines: Vec<String>,

    // ---- Watches and exceptions ----
    pub watches: Vec<Watch>,
    /// Exception filters the adapter offered (kept across sessions so the
    /// user's choice survives a restart).
    pub exception_filters: Vec<ExceptionFilter>,

    // ---- UI ----
    /// Whether debug panels are visible.
    pub panels_visible: bool,
    /// The user opened the panel themselves (`<Space>dv` / `<Space>df`): it
    /// stays when a session ends instead of disappearing with it.
    pub panel_pinned: bool,
    pub panel: PanelUi,

    // ---- Execution line tracking ----
    /// Current execution file path (for gutter indicator).
    pub execution_file: Option<PathBuf>,
    /// Current execution line (1-based, for gutter indicator).
    pub execution_line: Option<u64>,
}

impl Default for DebugState {
    fn default() -> Self {
        Self::new()
    }
}

impl DebugState {
    pub fn new() -> Self {
        Self {
            session_active: false,
            is_running: false,
            stopped_thread: None,
            event_thread: None,
            threads: Vec::new(),
            stop_reason: None,
            exception: None,
            breakpoints: HashMap::new(),
            stack_frames: Vec::new(),
            selected_frame: 0,
            scopes: Vec::new(),
            variables: HashMap::new(),
            expanded_refs: HashSet::new(),
            output_lines: Vec::new(),
            watches: Vec::new(),
            exception_filters: Vec::new(),
            panels_visible: false,
            panel_pinned: false,
            panel: PanelUi::default(),
            execution_file: None,
            execution_line: None,
        }
    }

    /// Toggle a breakpoint at the given line in the given file.
    /// Returns the new set of breakpoint lines for that file.
    pub fn toggle_breakpoint(&mut self, path: &Path, line: u64) -> Vec<u64> {
        let entry = self.breakpoints.entry(path.to_path_buf()).or_default();

        if let Some(idx) = entry.iter().position(|bp| bp.line == line) {
            entry.remove(idx);
        } else {
            entry.push(BreakpointState {
                line,
                verified: false,
                id: None,
                condition: None,
                log_message: None,
                hit_condition: None,
                enabled: true,
            });
        }

        entry.iter().map(|bp| bp.line).collect()
    }

    /// Get breakpoint lines for a file.
    pub fn breakpoint_lines(&self, path: &Path) -> Vec<u64> {
        self.breakpoints
            .get(path)
            .map(|bps| bps.iter().map(|bp| bp.line).collect())
            .unwrap_or_default()
    }

    /// Lines the adapter should know about (enabled breakpoints only).
    pub fn enabled_breakpoint_lines(&self, path: &Path) -> Vec<u64> {
        self.breakpoints
            .get(path)
            .map(|bps| {
                bps.iter()
                    .filter(|bp| bp.enabled)
                    .map(|bp| bp.line)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether the breakpoint at `line` exists and is enabled.
    pub fn is_breakpoint_enabled(&self, path: &Path, line: u64) -> bool {
        self.breakpoints
            .get(path)
            .is_some_and(|bps| bps.iter().any(|bp| bp.line == line && bp.enabled))
    }

    /// Removes a breakpoint. Returns whether one existed.
    pub fn remove_breakpoint(&mut self, path: &Path, line: u64) -> bool {
        let Some(entry) = self.breakpoints.get_mut(path) else {
            return false;
        };
        let before = entry.len();
        entry.retain(|bp| bp.line != line);
        // The (now empty) entry stays so the next sync tells the adapter
        // that the file has no breakpoints any more.
        entry.len() != before
    }

    /// Enables or disables a breakpoint. Returns the new state, or `None`
    /// when there is no breakpoint at that line.
    pub fn toggle_breakpoint_enabled(&mut self, path: &Path, line: u64) -> Option<bool> {
        let bp = self
            .breakpoints
            .get_mut(path)?
            .iter_mut()
            .find(|bp| bp.line == line)?;
        bp.enabled = !bp.enabled;
        Some(bp.enabled)
    }

    /// Every breakpoint, ordered by file and line.
    pub fn all_breakpoints(&self) -> Vec<(&Path, &BreakpointState)> {
        let mut all: Vec<(&Path, &BreakpointState)> = self
            .breakpoints
            .iter()
            .flat_map(|(path, bps)| bps.iter().map(move |bp| (path.as_path(), bp)))
            .collect();
        all.sort_by(|a, b| a.0.cmp(b.0).then(a.1.line.cmp(&b.1.line)));
        all
    }

    /// Check if a line has a breakpoint.
    pub fn has_breakpoint(&self, path: &Path, line: u64) -> bool {
        self.breakpoints
            .get(path)
            .is_some_and(|bps| bps.iter().any(|bp| bp.line == line))
    }

    /// Update breakpoints with responses from the debug adapter.
    pub fn update_breakpoints(&mut self, path: &Path, dap_bps: &[DapBreakpoint]) {
        let entry = self.breakpoints.entry(path.to_path_buf()).or_default();
        let old_entries = entry.clone();
        entry.clear();
        // Disabled breakpoints were not sent, so the reply knows nothing of them.
        entry.extend(old_entries.iter().filter(|bp| !bp.enabled).cloned());
        for bp in dap_bps {
            if let Some(line) = bp.line {
                // Keep what the user attached to the breakpoint (the reply
                // knows nothing of it).
                let old = old_entries.iter().find(|old| old.line == line);
                entry.push(BreakpointState {
                    line,
                    verified: bp.verified,
                    id: bp.id,
                    condition: old.and_then(|o| o.condition.clone()),
                    log_message: old.and_then(|o| o.log_message.clone()),
                    hit_condition: old.and_then(|o| o.hit_condition.clone()),
                    enabled: true,
                });
            }
        }
        entry.sort_by_key(|bp| bp.line);
    }

    /// Update the execution position from the selected stack frame.
    pub fn update_execution_position(&mut self) {
        if let Some(frame) = self.stack_frames.get(self.selected_frame) {
            self.execution_line = Some(frame.line);
            self.execution_file = frame
                .source
                .as_ref()
                .and_then(|s| s.path.as_ref())
                .map(PathBuf::from);
        } else {
            self.execution_line = None;
            self.execution_file = None;
        }
    }

    /// Set a condition on a breakpoint. If the breakpoint doesn't exist, creates it.
    pub fn set_breakpoint_condition(&mut self, path: &Path, line: u64, condition: Option<String>) {
        self.edit_breakpoint(path, line, |bp| bp.condition = condition);
    }

    /// Makes the breakpoint at `line` a logpoint (`None` makes it a plain
    /// breakpoint again). Creates the breakpoint when there is none.
    pub fn set_breakpoint_log_message(&mut self, path: &Path, line: u64, message: Option<String>) {
        self.edit_breakpoint(path, line, |bp| bp.log_message = message);
    }

    /// Sets the hit-count condition of the breakpoint at `line`.
    pub fn set_breakpoint_hit_condition(
        &mut self,
        path: &Path,
        line: u64,
        hit_condition: Option<String>,
    ) {
        self.edit_breakpoint(path, line, |bp| bp.hit_condition = hit_condition);
    }

    fn edit_breakpoint(&mut self, path: &Path, line: u64, edit: impl FnOnce(&mut BreakpointState)) {
        let entry = self.breakpoints.entry(path.to_path_buf()).or_default();
        if let Some(bp) = entry.iter_mut().find(|bp| bp.line == line) {
            edit(bp);
        } else {
            let mut bp = BreakpointState {
                line,
                verified: false,
                id: None,
                condition: None,
                log_message: None,
                hit_condition: None,
                enabled: true,
            };
            edit(&mut bp);
            entry.push(bp);
            entry.sort_by_key(|bp| bp.line);
        }
    }

    /// The breakpoint at `line`, if any.
    pub fn breakpoint_at(&self, path: &Path, line: u64) -> Option<&BreakpointState> {
        self.breakpoints
            .get(path)?
            .iter()
            .find(|bp| bp.line == line)
    }

    /// Get the condition for a breakpoint at a given line, if any.
    pub fn breakpoint_condition(&self, path: &Path, line: u64) -> Option<&str> {
        self.breakpoints
            .get(path)
            .and_then(|bps| bps.iter().find(|bp| bp.line == line))
            .and_then(|bp| bp.condition.as_deref())
    }

    /// Check if a breakpoint at the given line is conditional.
    pub fn is_conditional_breakpoint(&self, path: &Path, line: u64) -> bool {
        self.breakpoint_condition(path, line).is_some()
    }

    /// Clear all live debug state (on session end).
    ///
    /// Output lines are deliberately kept: they are the only record of why a
    /// program ended. They are reset when the next session starts.
    pub fn clear(&mut self) {
        self.end_session_keep_output();
        // Keep breakpoints — they persist across sessions.
    }

    /// Everything tied to a live debuggee goes: session flag, stop state,
    /// frames, variables and the execution marker.
    pub fn end_session_keep_output(&mut self) {
        self.session_active = false;
        self.is_running = false;
        self.stopped_thread = None;
        self.event_thread = None;
        self.threads.clear();
        self.stop_reason = None;
        self.exception = None;
        self.stack_frames.clear();
        self.selected_frame = 0;
        self.scopes.clear();
        self.variables.clear();
        self.expanded_refs.clear();
        self.execution_file = None;
        self.execution_line = None;
        self.clear_watch_values();
    }

    /// Forgets watch results (they belong to a stop that no longer exists);
    /// the expressions stay.
    pub fn clear_watch_values(&mut self) {
        for watch in &mut self.watches {
            watch.result = None;
            watch.type_ = None;
            watch.variables_reference = 0;
        }
    }
}
