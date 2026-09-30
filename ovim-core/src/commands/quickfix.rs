//! Quickfix commands.

use super::Ex;
use crate::command_result::{err, ok, CommandResult};
use crate::editor::{Editor, QuickfixEntry};

/// `:cdo {cmd}` runs `cmd` at every quickfix entry, `:cfdo {cmd}` at the
/// first entry of every file. Stops at the first error like vim.
pub(super) fn quickfix_do(editor: &mut Editor, ex: &Ex) -> CommandResult {
    let per_file = ex.command.names[0] == "cfdo";
    let name = if per_file { "cfdo" } else { "cdo" };
    if ex.args.is_empty() {
        return err("E471: Argument required");
    }
    let entries: Vec<(usize, QuickfixEntry)> = {
        let mut seen = std::collections::HashSet::new();
        editor
            .quickfix_list()
            .entries()
            .iter()
            .cloned()
            .enumerate()
            .filter(|(_, entry)| {
                entry
                    .filename
                    .as_ref()
                    .is_some_and(|file| !per_file || seen.insert(file.clone()))
            })
            .collect()
    };
    if entries.is_empty() {
        return err("E42: No Errors");
    }
    let total = entries.len();
    let mut done = 0;
    for (index, entry) in entries {
        editor.quickfix_list_mut().set_selected(index);
        if let CommandResult::Error(error) = super::jump_to_quickfix_entry(editor, &entry) {
            return err(format!("{name}: {}", error.error));
        }
        if let CommandResult::Error(error) = super::run_line(editor, ex.args) {
            return err(format!(
                "{name}: stopped at entry {} of {total}: {}",
                index + 1,
                error.error
            ));
        }
        done += 1;
    }
    ok(format!(
        "{name}: ran on {done} {}",
        if per_file { "file(s)" } else { "entr(ies)" }
    ))
}
