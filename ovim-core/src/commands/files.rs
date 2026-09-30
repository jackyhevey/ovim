//! Writing buffers: `:w`, `:w {file}`, `:w !cmd`.

use super::Ex;
use crate::command_result::{err, ok, CommandResult};
use crate::editor::Editor;

/// Expands `~` to the home directory.
pub(crate) fn expand_tilde(path: &str) -> Result<std::path::PathBuf, String> {
    if path == "~" || path.starts_with("~/") {
        let home = dirs::home_dir().ok_or("Could not determine home directory")?;
        return Ok(if path == "~" {
            home
        } else {
            home.join(&path[2..])
        });
    }
    Ok(std::path::PathBuf::from(path))
}

/// Options for [`save_buffer`].
pub(super) struct SaveOpts<'a> {
    /// Path to save to (None = the buffer's own file).
    pub path: Option<&'a str>,
    /// Skip the read-only and changed-on-disk checks.
    pub force: bool,
    /// Quit the editor after a successful save.
    pub quit_after: bool,
}

fn same_file(a: &str, b: &str) -> bool {
    let (a, b) = (std::path::Path::new(a), std::path::Path::new(b));
    a == b
        || match (a.canonicalize(), b.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
}

fn written(path: &str, editor: &Editor) -> String {
    format!(
        "\"{}\" {}L, {}C written",
        path,
        editor.buffer().rope().len_lines(),
        editor.buffer().rope().len_chars()
    )
}

/// Save the buffer to its file, or make `opts.path` its file.
pub(super) fn save_buffer(editor: &mut Editor, opts: SaveOpts<'_>) -> CommandResult {
    if !opts.force && editor.buffer().is_read_only() {
        return err("E45: 'readonly' option is set (add ! to override)");
    }

    let resolved = match opts.path {
        Some(raw) => match expand_tilde(raw) {
            Ok(p) => p.to_string_lossy().to_string(),
            Err(e) => return err(format!("Failed to expand path '{}': {}", raw, e)),
        },
        None => match editor.buffer().file_path().map(|s| s.to_string()) {
            Some(p) => p,
            None => return err("No file name"),
        },
    };

    let old_path = editor.buffer().file_path().map(|s| s.to_string());

    // Do not silently overwrite changes made by another process. Save-as to a
    // different file remains valid, and the bang variants are the explicit
    // escape hatch when the user intentionally wants the in-memory copy to win.
    let targets_current_file = old_path
        .as_deref()
        .is_some_and(|current| same_file(current, &resolved));
    if !opts.force && targets_current_file && editor.buffer().file_mtime().is_some() {
        match editor.buffer().check_external_modification() {
            Ok(true) => return err("E211: File changed since editing started (add ! to override)"),
            Ok(false) => {}
            Err(error) => return err(format!("Failed to check file before saving: {error}")),
        }
    }

    match editor.buffer_mut().save_as(&resolved) {
        Ok(_) => {
            let new_path = editor.buffer().file_path().map(|s| s.to_string());
            editor.handle_file_path_transition_after_save(old_path, new_path);
            // Git refresh runs on a background thread to avoid blocking the UI.
            editor.spawn_git_refresh(&resolved, editor.options.blame);
            if opts.force {
                editor.buffer_mut().set_read_only(false);
            }
            editor.mark_saved();
            editor.mark_buffer_saved();

            if opts.quit_after {
                editor.quit();
                return ok("Saved and quitting");
            }

            let saved_path = editor
                .buffer()
                .file_path()
                .map(|p| p.to_string())
                .unwrap_or(resolved);
            ok(written(&saved_path, editor))
        }
        Err(e) => err(format!("Failed to save: {}", e)),
    }
}

/// `:[range]w[rite][!] [file]` and `:[range]w[rite] !{cmd}`.
///
/// Like vim, `:w {file}` on a named buffer writes a copy and keeps editing
/// the buffer's own file (`:saveas` renames); an unnamed buffer takes the
/// name. An existing other file needs `!` (E13).
pub(super) fn write(editor: &mut Editor, ex: &Ex) -> CommandResult {
    if let Some(command) = ex.args.strip_prefix('!') {
        let range = ex.range.expect("whole-buffer default");
        return super::shell::write_to_command(editor, range, command.trim());
    }
    if ex.explicit_range {
        return err("E140: Use ! to write partial buffer");
    }
    let current = editor.buffer().file_path().map(str::to_string);
    let target = match (ex.args, &current) {
        ("", _) => None,
        (file, Some(current)) => {
            let path = match expand_tilde(file) {
                Ok(path) => path,
                Err(error) => return err(error),
            };
            let path = path.to_string_lossy().to_string();
            (!same_file(current, &path)).then_some(path)
        }
        // An unnamed buffer takes the name.
        (file, None) => {
            return save_buffer(
                editor,
                SaveOpts {
                    path: Some(file),
                    force: ex.bang,
                    quit_after: false,
                },
            )
        }
    };
    let Some(target) = target else {
        return save_buffer(
            editor,
            SaveOpts {
                path: None,
                force: ex.bang,
                quit_after: false,
            },
        );
    };
    if !ex.bang && std::path::Path::new(&target).exists() {
        return err("E13: File exists (add ! to override)");
    }
    match editor.buffer().write_copy(&target) {
        Ok(()) => ok(written(&target, editor)),
        Err(error) => err(format!("Failed to save: {error}")),
    }
}
