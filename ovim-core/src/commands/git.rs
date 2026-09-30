//! Git commands that take arguments: the branch diff review and the
//! views opened from the git pickers. The argument-free ones (`:GitStatus`,
//! `:GitStage`, `:ConflictNext`, ...) are one-line table entries.

use super::Ex;
use crate::command_result::{err, ok_silent, CommandResult};
use crate::editor::Editor;

/// `:GitDiff [base]`: open (or return to) the branch diff review against
/// the default branch or an explicit ref.
pub(super) fn diff_review(editor: &mut Editor, ex: &Ex) -> CommandResult {
    match editor.open_diff_review((!ex.args.is_empty()).then_some(ex.args)) {
        Ok(()) => ok_silent(),
        Err(e) => err(format!("GitDiff: {e:#}")),
    }
}

/// `:GitDiffLayout [split|unified]` (no argument toggles).
pub(super) fn diff_layout(editor: &mut Editor, ex: &Ex) -> CommandResult {
    if ex.args.is_empty() {
        editor.toggle_diff_review_layout();
        return ok_silent();
    }
    match crate::editor::DiffLayout::parse(ex.args) {
        Some(layout) => {
            editor.set_diff_review_layout(layout);
            ok_silent()
        }
        None => err("GitDiffLayout: use split or unified"),
    }
}

/// `:GitDiffFile {path}`: the file's diff against HEAD.
pub(super) fn diff_file(editor: &mut Editor, ex: &Ex) -> CommandResult {
    if ex.args.is_empty() {
        return err("Usage: :GitDiffFile <path>");
    }
    match editor.git_show_file_diff(std::path::Path::new(ex.args)) {
        Ok(()) => ok_silent(),
        Err(error) => err(format!("GitDiffFile: {error:#}")),
    }
}

/// `:GitEdit {path}`: open a file from a git view.
pub(super) fn edit(editor: &mut Editor, ex: &Ex) -> CommandResult {
    if ex.args.is_empty() {
        return err("Usage: :GitEdit <path>");
    }
    match editor.load_file(ex.args) {
        Ok(()) => ok_silent(),
        Err(error) => err(format!("Failed to open {}: {error}", ex.args)),
    }
}

/// `:GitShow {commit} [path]`.
pub(super) fn show(editor: &mut Editor, ex: &Ex) -> CommandResult {
    if ex.args.is_empty() {
        return err("Usage: :GitShow <commit> [path]");
    }
    let (oid, path) = match ex.args.split_once(' ') {
        Some((oid, path)) => (oid, Some(path.trim())),
        None => (ex.args, None),
    };
    match editor.git_show_commit(oid, path) {
        Ok(()) => ok_silent(),
        Err(error) => err(format!("GitShow: {error:#}")),
    }
}
