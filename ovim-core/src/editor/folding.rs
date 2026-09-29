//! Code folding on the editor: `z` commands, automatic fold sources (LSP
//! `foldingRange`, indentation fallback), cursor rules around closed folds and
//! the `⋯ N lines` marker on fold headers.
//!
//! Folds are only computed once the user issues a fold command for a buffer
//! and are then kept fresh (debounced) as the text changes.

use super::decoration::{Decoration, DecorationPlacement, DecorationSource, DecorationStyle};
use super::Editor;
use crate::fold::indent_fold_ranges;
use crate::unicode::GraphemeCol;

/// How long the buffer must be quiet before folds are recomputed.
const FOLD_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(250);

impl Editor {
    /// Requests folds for the current buffer: indentation folds immediately
    /// (so `zM` works at once), the language server's ranges shortly after.
    pub(crate) fn ensure_folds(&mut self) {
        if self.buffer().fold_manager().is_active() {
            return;
        }
        self.buffer_mut().fold_manager_mut().activate();
        self.compute_indent_folds();
        self.lsp.intents.folding_ranges = true;
    }

    /// Replaces the automatic folds with indentation folds.
    fn compute_indent_folds(&mut self) {
        let tab_width = self.indent_options().tab_width.max(1);
        let buffer = self.buffer();
        let lines: Vec<String> = (0..buffer.line_count())
            .map(|line| buffer.line_text(line).unwrap_or_default().to_string())
            .collect();
        let ranges = indent_fold_ranges(&lines, tab_width);
        let version = buffer.version();
        let line_count = buffer.line_count();
        self.buffer_mut()
            .fold_manager_mut()
            .set_auto_folds(&ranges, line_count, version, false);
    }

    /// Applies folds from the language server. Returns false for stale or
    /// empty answers (the indentation folds stay).
    pub fn apply_lsp_folding_ranges(
        &mut self,
        file_path: &str,
        buffer_version: usize,
        ranges: &[lsp_types::FoldingRange],
    ) -> bool {
        if self.buffer().file_path() != Some(file_path) || self.buffer().version() != buffer_version
        {
            return false;
        }
        if ranges.is_empty() {
            return false;
        }
        let pairs: Vec<(usize, usize)> = ranges
            .iter()
            .map(|range| (range.start_line as usize, range.end_line as usize))
            .collect();
        let line_count = self.buffer().line_count();
        self.buffer_mut().fold_manager_mut().set_auto_folds(
            &pairs,
            line_count,
            buffer_version,
            true,
        );
        self.refresh_fold_view();
        true
    }

    /// Recomputes the automatic folds after the text settled.
    pub(crate) async fn maintain_folds(&mut self) {
        let buffer = self.buffer();
        let manager = buffer.fold_manager();
        if !manager.is_active() {
            return;
        }
        let version = buffer.version();
        if manager.auto_version() == Some(version) {
            self.lsp.state.fold_tracking = None;
            return;
        }
        match self.lsp.state.fold_tracking {
            Some((tracked, since)) if tracked == version => {
                if since.elapsed() < FOLD_DEBOUNCE {
                    return;
                }
            }
            _ => {
                self.lsp.state.fold_tracking = Some((version, std::time::Instant::now()));
                return;
            }
        }
        self.lsp.state.fold_tracking = None;
        if !self.buffer().fold_manager().is_lsp_backed() {
            self.compute_indent_folds();
            self.refresh_fold_view();
        }
        self.request_folding_ranges().await;
    }

    pub(in crate::editor) async fn request_folding_ranges(&mut self) {
        let Some(lsp) = self.lsp.state.lsp_manager.clone() else {
            return;
        };
        let Some(file_path) = self.buffer().file_path().map(|p| p.to_string()) else {
            return;
        };
        let Some(language_id) = self.language_id_for_path(&file_path) else {
            return;
        };
        let Some(uri) = crate::lsp::uri_from_file_path(&file_path) else {
            return;
        };
        let version = self.buffer().version();
        self.ensure_lsp_document_synced().await;
        let (tx, rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let result = lsp.folding_range(&uri, &language_id).await;
            let _ = tx.send(
                result.map(|ranges| crate::editor::lsp_slot::FoldingRangesResult {
                    ranges,
                    file_path,
                    buffer_version: version,
                }),
            );
        });
        self.lsp.slots.folding_ranges.fire(task, rx);
    }

    pub(in crate::editor) fn poll_folding_slot(&mut self) -> bool {
        let Some(result) = self
            .lsp
            .slots
            .folding_ranges
            .poll_with_timeout(std::time::Duration::from_secs(30))
        else {
            return false;
        };
        match result {
            Ok(result) => {
                let applied = self.apply_lsp_folding_ranges(
                    &result.file_path,
                    result.buffer_version,
                    &result.ranges,
                );
                if applied {
                    self.mark_dirty();
                }
                applied
            }
            Err(_) => false,
        }
    }

    // ----- `z` commands ---------------------------------------------------------

    /// Handles `z{key}` fold commands. Returns false when `key` is not a fold
    /// command (so the caller can try the scroll commands).
    pub fn fold_command(&mut self, key: char) -> bool {
        if !matches!(
            key,
            'o' | 'c'
                | 'a'
                | 'O'
                | 'C'
                | 'A'
                | 'R'
                | 'M'
                | 'v'
                | 'n'
                | 'N'
                | 'i'
                | 'd'
                | 'D'
                | 'E'
                | 'j'
                | 'k'
        ) {
            return false;
        }
        self.ensure_folds();
        let line = self.buffer().cursor().line();
        let manager = self.buffer_mut().fold_manager_mut();
        match key {
            'o' => {
                manager.open_one(line);
            }
            'c' => {
                manager.close_one(line);
            }
            'a' => {
                manager.toggle_one(line);
            }
            'O' => {
                manager.open_recursive(line);
            }
            'C' => {
                manager.close_recursive(line);
            }
            'A' => {
                manager.toggle_recursive(line);
            }
            'R' => manager.open_all(),
            'M' => {
                manager.set_enabled(true);
                manager.close_all();
            }
            'v' => {
                manager.reveal(line);
            }
            'n' => manager.set_enabled(false),
            'N' => manager.set_enabled(true),
            'i' => {
                let enabled = manager.is_enabled();
                manager.set_enabled(!enabled);
            }
            'd' => manager.delete_fold_at(line),
            'D' => manager.delete_recursive(line),
            'E' => manager.delete_all(),
            'j' | 'k' => {
                let target = if key == 'j' {
                    manager.next_fold_start(line)
                } else {
                    manager.previous_fold_end(line)
                };
                if let Some(target) = target {
                    self.move_cursor_to_line_keeping_column(target);
                }
            }
            _ => {}
        }
        self.clear_count();
        self.after_fold_change();
        true
    }

    /// `[z` / `]z`: start / end of the open fold containing the cursor.
    pub fn fold_edge_motion(&mut self, to_end: bool) {
        self.ensure_folds();
        let line = self.buffer().cursor().line();
        if let Some((start, end)) = self.buffer().fold_manager().innermost_open_fold(line) {
            let target = if to_end { end } else { start };
            if target != line {
                self.move_cursor_to_line_keeping_column(target);
            }
        }
        self.clear_count();
        self.after_fold_change();
    }

    fn move_cursor_to_line_keeping_column(&mut self, line: usize) {
        let col = self.buffer().cursor().col();
        self.buffer_mut().cursor_mut().set_position(line, col);
        self.buffer_mut().validate_cursor_position();
    }

    /// Cursor / scroll / marker bookkeeping after any fold state change.
    fn after_fold_change(&mut self) {
        let line = self.buffer().cursor().line();
        self.settle_cursor_after_fold_change(line);
        self.refresh_fold_view();
        self.mark_dirty();
    }

    /// A cursor inside a just-closed fold moves to the fold's header.
    fn settle_cursor_after_fold_change(&mut self, line: usize) {
        if let Some((start, _)) = self.buffer().fold_manager().closed_fold_at(line) {
            if start != line {
                self.move_cursor_to_line_keeping_column(start);
            }
        }
    }

    // ----- per-key bookkeeping ---------------------------------------------------

    /// Runs after every key: keeps fold ranges aligned with line-count changes,
    /// keeps the cursor out of closed folds (Vim's rules) and refreshes the
    /// header markers.
    pub(crate) fn sync_folds_after_key(&mut self, prev_line: usize, prev_col: GraphemeCol) {
        if self.buffer().fold_manager().is_empty() {
            self.clear_fold_markers();
            return;
        }
        let line_count = self.buffer().line_count();
        self.buffer_mut()
            .fold_manager_mut()
            .adjust_for_line_count(line_count, prev_line);

        let cursor = self.buffer().cursor();
        let (line, col) = (cursor.line(), cursor.col());
        let insert_like = matches!(
            self.mode(),
            crate::mode::Mode::Insert | crate::mode::Mode::Replace
        );
        match self.buffer().fold_manager().closed_fold_at(line) {
            Some((start, end)) if line > start => {
                if insert_like {
                    // Typing into a hidden line opens the fold around it.
                    self.buffer_mut().fold_manager_mut().reveal(line);
                } else if prev_line == start && line == start + 1 {
                    // `j` from a closed header: on to the next visible line.
                    let target = end + 1;
                    if target < line_count {
                        self.move_cursor_to_line_keeping_column(target);
                    } else {
                        self.move_cursor_to_line_keeping_column(start);
                    }
                } else if line.abs_diff(prev_line) <= 1 {
                    self.move_cursor_to_line_keeping_column(start);
                } else {
                    // A jump (search, mark, `%`, `G`, ...) lands in the fold:
                    // open it so the target is visible.
                    self.buffer_mut().fold_manager_mut().reveal(line);
                }
            }
            Some((start, _)) if line == start && line == prev_line && col != prev_col => {
                // Horizontal movement on a closed header opens it (`l`, `$`, ...).
                if !insert_like {
                    self.buffer_mut().fold_manager_mut().open_one(line);
                }
            }
            _ => {}
        }
        // The viewport must not start inside a hidden region.
        let top = self.scroll_offset();
        if let Some((start, _)) = self.buffer().fold_manager().closed_fold_at(top) {
            if start != top {
                self.set_scroll_offset_line(start);
            }
        }
        self.refresh_fold_view();
    }

    fn set_scroll_offset_line(&mut self, line: usize) {
        self.viewport.scroll_offset = line;
        self.viewport.scroll_subrow = 0;
        if let Some(window) = self
            .window_manager
            .as_mut()
            .and_then(|wm| wm.focused_window_mut())
        {
            window.set_scroll_position(line, 0);
        }
    }

    fn clear_fold_markers(&mut self) {
        if self.lsp.state.fold_markers.is_empty() {
            return;
        }
        let rope = self.buffer().rope().clone();
        self.decorations
            .replace_source(DecorationSource::Fold, Vec::new(), &rope);
        self.lsp.state.fold_markers.clear();
        self.mark_dirty();
    }

    /// Re-renders the `⋯ N lines` markers when the set of closed folds changed.
    pub(crate) fn refresh_fold_view(&mut self) {
        let manager = self.buffer().fold_manager();
        let mut markers: Vec<(usize, usize)> = manager
            .folds()
            .iter()
            .filter_map(|fold| {
                manager
                    .folded_line_count_at(fold.start_line())
                    .map(|count| (fold.start_line(), count))
            })
            .collect();
        markers.dedup();
        if markers == self.lsp.state.fold_markers {
            return;
        }
        let rope = self.buffer().rope().clone();
        let version = self.buffer().version() as u64;
        let decorations: Vec<Decoration> = markers
            .iter()
            .filter(|(line, _)| *line < rope.len_lines())
            .map(|&(line, count)| {
                let text = format!("  ⋯ {count} lines");
                Decoration {
                    placement: DecorationPlacement::EndOfLine {
                        char_offset: rope.line_to_char(line),
                    },
                    source: DecorationSource::Fold,
                    display_width: crate::display::display_width(&text, 1),
                    text,
                    style: DecorationStyle::new(crate::color::Color::Gray).with_italic(),
                    priority: 5,
                    source_version: version,
                }
            })
            .collect();
        self.decorations
            .replace_source(DecorationSource::Fold, decorations, &rope);
        self.lsp.state.fold_markers = markers;
        self.mark_dirty();
    }

    /// The buffer line drawn on screen row `rel_row` (top of the viewport is
    /// 0) when lines are not soft-wrapped: closed folds take no rows.
    pub(crate) fn line_for_screen_row(&self, rel_row: usize) -> usize {
        let folds = self.buffer().fold_manager();
        folds.line_at_visible_index(folds.visible_index(self.scroll_offset()) + rel_row)
    }

    /// Line count a linewise command with `count` lines covers when closed
    /// folds count as one line each (`dd` on a closed fold deletes the whole
    /// fold, as in Vim).
    pub(crate) fn linewise_count_over_folds(&self, count: usize) -> usize {
        let manager = self.buffer().fold_manager();
        if manager.hidden_ranges().is_empty() {
            return count;
        }
        let start = self.buffer().cursor().line();
        let max_line = self.buffer().line_count().saturating_sub(1);
        let last_visible = manager.step_down(start, count.saturating_sub(1), max_line);
        let end = manager
            .closed_fold_at(last_visible)
            .map_or(last_visible, |(_, end)| end);
        end - start + 1
    }
}
