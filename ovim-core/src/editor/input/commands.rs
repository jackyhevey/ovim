use crate::command_result::CommandResult;
use crate::editor::path_completion::extract_path_from_command;
use crate::editor::{Editor, Mode};
use crate::{KeyCode, KeyEvent};
use anyhow::Result;

/// Handles input in Command mode
pub fn handle_command_mode(editor: &mut Editor, key_event: KeyEvent) -> Result<()> {
    if let Some(control) = super::helpers::prompt_control(&key_event) {
        use super::helpers::PromptControl;
        match control {
            PromptControl::Cancel => {
                return handle_command_mode(
                    editor,
                    KeyEvent::new(KeyCode::Esc, crate::Modifiers::NONE),
                );
            }
            PromptControl::Backspace => {
                return handle_command_mode(
                    editor,
                    KeyEvent::new(KeyCode::Backspace, crate::Modifiers::NONE),
                );
            }
            PromptControl::DeleteToStart => {
                editor.delete_command_line_to_start();
                update_path_completion(editor);
            }
            PromptControl::DeleteWord => {
                editor.delete_command_line_word();
                update_path_completion(editor);
            }
            PromptControl::Home => editor.move_command_cursor_home(),
            PromptControl::End => editor.move_command_cursor_end(),
            PromptControl::Ignore => {}
        }
        return Ok(());
    }
    match key_event.code {
        KeyCode::Char(ch) => {
            editor.append_to_command_line(ch);
            update_path_completion(editor);
        }
        KeyCode::Backspace => {
            if editor.command_line().is_empty() {
                editor.path_completion_mut().hide();
                editor.set_mode(Mode::Normal);
            } else {
                editor.backspace_command_line();
                update_path_completion(editor);
            }
        }
        KeyCode::Delete => {
            editor.delete_command_line_char();
            update_path_completion(editor);
        }
        KeyCode::Tab => {
            handle_tab_completion(editor, false);
        }
        KeyCode::BackTab => {
            handle_tab_completion(editor, true);
        }
        KeyCode::Up => {
            if editor.path_completion().is_visible() {
                editor.path_completion_mut().select_previous();
                accept_selected_into_command_line(editor);
            } else {
                editor.history_prev();
            }
        }
        KeyCode::Down => {
            if editor.path_completion().is_visible() {
                editor.path_completion_mut().select_next();
                accept_selected_into_command_line(editor);
            } else {
                editor.history_next();
            }
        }
        KeyCode::Left => {
            editor.move_command_cursor_left();
        }
        KeyCode::Right => {
            editor.move_command_cursor_right();
        }
        KeyCode::Home => {
            editor.move_command_cursor_home();
        }
        KeyCode::End => {
            editor.move_command_cursor_end();
        }
        KeyCode::Enter => {
            if editor.path_completion().is_visible() {
                if editor.path_completion().selected_is_dir() {
                    // Directory: accept into command line, refresh completions, don't execute.
                    if let Some(new_path) = editor.path_completion().accept() {
                        let cmd = editor.command_line().to_string();
                        if let Some(path_portion) = extract_path_from_command(&cmd) {
                            let prefix_len = cmd.len() - path_portion.len();
                            let new_cmd = format!("{}{}", &cmd[..prefix_len], new_path);
                            editor.set_command_line(&new_cmd);
                            let cwd = std::env::current_dir().unwrap_or_default();
                            editor.path_completion_mut().update(&new_path, &cwd);
                        }
                    }
                } else {
                    // File: accept into command line, then execute.
                    if let Some(new_path) = editor.path_completion().accept() {
                        let cmd = editor.command_line().to_string();
                        if let Some(path_portion) = extract_path_from_command(&cmd) {
                            let prefix_len = cmd.len() - path_portion.len();
                            let new_cmd = format!("{}{}", &cmd[..prefix_len], new_path);
                            editor.set_command_line(&new_cmd);
                        }
                    }
                    editor.path_completion_mut().hide();
                    editor.add_command_to_history();
                    execute_command(editor)?;
                    editor.clear_command_line();
                    if editor.mode() == Mode::Command {
                        editor.set_mode(Mode::Normal);
                    }
                }
            } else {
                editor.add_command_to_history();
                execute_command(editor)?;
                editor.clear_command_line();
                if editor.mode() == Mode::Command {
                    editor.set_mode(Mode::Normal);
                }
            }
        }
        KeyCode::Esc => {
            editor.path_completion_mut().hide();
            editor.clear_command_line();
            editor.set_mode(Mode::Normal);
        }
        _ => {}
    }
    Ok(())
}

/// Updates the path completion popup based on current command line content.
fn update_path_completion(editor: &mut Editor) {
    let cmd = editor.command_line().to_string();
    if let Some(path_portion) = extract_path_from_command(&cmd) {
        let path_portion = path_portion.to_string();
        let cwd = std::env::current_dir().unwrap_or_default();
        editor.path_completion_mut().update(&path_portion, &cwd);
    } else {
        editor.path_completion_mut().hide();
    }
}

/// Known command names for Tab completion.
const COMMAND_NAMES: &[&str] = &[
    "Problems",
    "GitStatus",
    "GitStage",
    "GitUnstage",
    "GitStageHunk",
    "GitUnstageHunk",
    "GitStageAll",
    "GitCommit",
    "GitAmend",
    "GitLog",
    "GitLogAll",
    "GitLineLog",
    "ConflictNext",
    "ConflictPrev",
    "ConflictOurs",
    "ConflictTheirs",
    "ConflictBoth",
    "ConflictNone",
    "Outline",
    "Recent",
    "Buffers",
    "Symbols",
    "SearchReplace",
    "ReplaceApply",
    "ReplaceUndo",
    "cdo",
    "cfdo",
    "grep",
    "update",
    "bd",
    "bdelete",
    "browser",
    "buffer",
    "buffers",
    "cd",
    "cclose",
    "cfirst",
    "clast",
    "close",
    "cn",
    "cnext",
    "copen",
    "colorscheme",
    "cp",
    "cprev",
    "cq",
    "cquit",
    "delmarks",
    "e",
    "edit",
    "f",
    "file",
    "help",
    "hi",
    "highlight",
    "history",
    "lcd",
    "ls",
    "LspInstall",
    "LspManager",
    "map",
    "marks",
    "messages",
    "nmap",
    "nohlsearch",
    "noh",
    "norm",
    "normal",
    "noremap",
    "nmapclear",
    "only",
    "pwd",
    "q",
    "qa",
    "quit",
    "quitall",
    "reg",
    "registers",
    "reload",
    "saveas",
    "se",
    "set",
    "unset",
    "session",
    "sort",
    "source",
    "sp",
    "split",
    "tabe",
    "tabedit",
    "tabclose",
    "tabmove",
    "tabnext",
    "tabprev",
    "unmap",
    "unlet",
    "vmap",
    "vsp",
    "vsplit",
    "w",
    "wa",
    "wall",
    "wq",
    "wqa",
    "write",
    "writeall",
    "x",
    "xa",
];

/// State for command name Tab completion cycling.
struct CmdCompletion {
    matches: Vec<&'static str>,
    index: usize,
    original_prefix: String,
}

thread_local! {
    static CMD_COMPLETION: std::cell::RefCell<Option<CmdCompletion>> = const { std::cell::RefCell::new(None) };
}

/// Handles Tab (forward=false) or BackTab (backward=true) for path completion.
fn handle_tab_completion(editor: &mut Editor, backward: bool) {
    let cmd = editor.command_line().to_string();
    let trimmed = cmd.trim_start();

    // If the command line has no space, we're completing a command name.
    if !trimmed.contains(' ') {
        handle_command_name_completion(editor, trimmed, backward);
        return;
    }

    // Otherwise, fall through to path completion.
    if !editor.path_completion().is_visible() {
        // Try to trigger completion from current command line.
        update_path_completion(editor);
        if !editor.path_completion().is_visible() {
            return;
        }
        // First Tab after triggering: accept entry[0] without cycling.
    } else if editor.path_completion().tab_accepted() {
        // Already visible and Tab was used before — cycle selection.
        if backward {
            editor.path_completion_mut().select_previous();
        } else {
            editor.path_completion_mut().select_next();
        }
    }
    // else: popup was visible from typing but Tab hasn't accepted yet —
    // accept the current selection (entry[0]) without cycling.

    editor.path_completion_mut().set_tab_accepted();

    // Accept the selected entry: replace path portion in command line.
    let accepted = editor.path_completion().accept();
    if let Some(new_path) = accepted {
        let cmd = editor.command_line().to_string();
        if let Some(path_portion) = extract_path_from_command(&cmd) {
            let prefix_len = cmd.len() - path_portion.len();
            let new_cmd = format!("{}{}", &cmd[..prefix_len], new_path);
            editor.set_command_line(&new_cmd);

            // If we just completed a directory, refresh entries for its contents.
            if new_path.ends_with('/') {
                let cwd = std::env::current_dir().unwrap_or_default();
                editor.path_completion_mut().update(&new_path, &cwd);
            }
        }
    }
}

/// Handle Tab completion for command names (first word of command line).
fn handle_command_name_completion(editor: &mut Editor, prefix: &str, backward: bool) {
    CMD_COMPLETION.with(|cell| {
        let mut state = cell.borrow_mut();

        // Check if we're continuing a previous completion cycle
        let continuing = state.as_ref().is_some_and(|s| {
            s.original_prefix == prefix || {
                // Also continue if the command line matches a previous completion result
                s.matches.contains(&prefix)
            }
        });

        if continuing {
            let s = state.as_mut().unwrap();
            if backward {
                s.index = if s.index == 0 {
                    s.matches.len() // wraps to original prefix
                } else {
                    s.index - 1
                };
            } else {
                s.index += 1;
                if s.index > s.matches.len() {
                    s.index = 0;
                }
            }
            // index == matches.len() means show original prefix
            if s.index == s.matches.len() {
                editor.set_command_line(&s.original_prefix);
            } else {
                editor.set_command_line(s.matches[s.index]);
            }
        } else {
            // Build new completion list
            let matches: Vec<&'static str> = COMMAND_NAMES
                .iter()
                .filter(|cmd| cmd.starts_with(prefix))
                .copied()
                .collect();

            if matches.is_empty() {
                *state = None;
                return;
            }

            if matches.len() == 1 {
                // Unique match — complete it directly
                editor.set_command_line(matches[0]);
                *state = None;
                return;
            }

            // Multiple matches — start cycling from first
            let idx = if backward { matches.len() - 1 } else { 0 };
            editor.set_command_line(matches[idx]);
            *state = Some(CmdCompletion {
                matches,
                index: idx,
                original_prefix: prefix.to_string(),
            });
        }
    });
}

/// Accepts the currently selected path completion entry and updates the command line text.
fn accept_selected_into_command_line(editor: &mut Editor) {
    if let Some(new_path) = editor.path_completion().accept() {
        let cmd = editor.command_line().to_string();
        if let Some(path_portion) = extract_path_from_command(&cmd) {
            let prefix_len = cmd.len() - path_portion.len();
            let new_cmd = format!("{}{}", &cmd[..prefix_len], new_path);
            editor.set_command_line(&new_cmd);
        }
    }
}

/// Executes a command string directly (used for API/Lua commands)
pub fn execute_command_string(editor: &mut Editor, command: &str) -> Result<()> {
    editor.with_execution_scope(|editor| execute_command_impl(editor, command))
}

/// Executes a command string on behalf of the headless API / CLI, returning a
/// structured [`CommandResult`].
///
/// The headless `exec` path historically called [`crate::commands::execute_command`]
/// directly, which only knows the "standard" ex-commands (`:w`, `:q`, `:set`, …).
/// Substitute (`:s`), global (`:g`/`:v`), ranges, and `:d`/`:y` live in the
/// interactive command handler and were therefore unreachable headlessly — an
/// `exec ':%s/a/b/'` returned "Not an editor command" while the same keys typed
/// interactively worked. Routing through the same dispatcher the interactive
/// command line uses keeps the two paths in parity.
pub fn execute_command_string_api(editor: &mut Editor, command: &str) -> CommandResult {
    editor.with_execution_scope(|editor| execute_command_string_api_inner(editor, command))
}

fn execute_command_string_api_inner(editor: &mut Editor, command: &str) -> CommandResult {
    let result = execute_command_string_api_legacy(editor, command);
    crate::commands::run_queued_without_terminal(editor, result)
}

fn execute_command_string_api_legacy(editor: &mut Editor, command: &str) -> CommandResult {
    use crate::command_result::{err, ok, ok_silent};

    let command = command.trim();

    // Standard commands return a structured result we forward verbatim — this
    // preserves messages like line counts and errors like "No write since last
    // change". Only when the standard dispatcher reports the command as unknown
    // do we fall through to the richer interactive handler.
    let result = crate::commands::execute_command(editor, command);
    let is_unknown = matches!(
        &result,
        CommandResult::Error(e) if e.error.contains("Not an editor command")
    );
    if !is_unknown {
        return result;
    }

    // The standard dispatcher performs no mutation when it doesn't recognize a
    // command, so it is safe to re-run the full interactive handler (which
    // re-checks the standard dispatcher and then handles substitute / global /
    // range / etc.). That handler reports outcomes on the status line rather
    // than returning them, so clear the line first and read it back afterwards.
    editor.set_status_message(String::new());
    match execute_command_string(editor, command) {
        Ok(()) => {
            let status = editor.status_message().trim().to_string();
            if status.is_empty() {
                ok_silent()
            } else if is_vim_error_status(&status) {
                err(status)
            } else {
                ok(status)
            }
        }
        Err(e) => err(e.to_string()),
    }
}

/// Vim surfaces command errors on the status line using the `E<number>:`
/// convention (`E146`, `E20`, `E486`, …). Map those back to an API error even
/// though the editor itself treats them as ordinary status messages.
fn is_vim_error_status(status: &str) -> bool {
    matches!(
        status.strip_prefix('E'),
        Some(rest) if rest.starts_with(|c: char| c.is_ascii_digit())
    )
}

/// Executes a command from the command line
fn execute_command(editor: &mut Editor) -> Result<()> {
    let command = editor.command_line().trim().to_string();
    execute_command_impl(editor, &command)
}

/// Internal command execution implementation
fn execute_command_impl(editor: &mut Editor, command: &str) -> Result<()> {
    let command = command.trim();

    // Handle command chaining with |
    // BUG FIX: Don't split on | for substitute, global, or vglobal commands
    // because | can appear in patterns like :s/foo|bar/baz/
    // `:s/pat/rep/flags | cmd` — the bar ends a substitute once its pattern and
    // replacement are complete (`:cfdo %s/a/b/ge | update`).
    if let Some((first, rest)) = split_after_substitute(command) {
        execute_command_impl(editor, first)?;
        return execute_command_impl(editor, rest);
    }

    if command.contains('|') && !command_owns_bar(command) {
        // Simple split for non-substitute commands
        for part in command.split('|') {
            let part = part.trim();
            if !part.is_empty() {
                execute_command_single(editor, part)?;
            }
        }
        return Ok(());
    }

    execute_command_single(editor, command)
}

/// Splits `command` at the bar that terminates a leading `:s` command.
///
/// Inside the pattern a bar is literal (regex alternation); in the replacement
/// and flags an unescaped bar separates the next Ex command, as in vim.
/// Returns `None` when `command` is not a substitute or has no such bar.
fn split_after_substitute(command: &str) -> Option<(&str, &str)> {
    let trimmed = command.trim();
    let range_len = trimmed
        .find(|c: char| !(c.is_ascii_digit() || ",%.$ '<>+-".contains(c)))
        .unwrap_or(trimmed.len());
    let body = &trimmed[range_len..];
    let mut chars = body.char_indices();
    let (_, first) = chars.next()?;
    if first != 's' {
        return None;
    }
    let (delim_at, delimiter) = chars.next()?;
    if delimiter.is_alphanumeric()
        || delimiter.is_whitespace()
        || matches!(delimiter, '|' | '"' | '\\')
    {
        return None;
    }
    let mut delimiters = 0;
    let mut escaped = false;
    for (offset, c) in body[delim_at..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' {
            escaped = true;
        } else if c == delimiter {
            delimiters += 1;
        } else if c == '|' && delimiters >= 2 {
            let at = range_len + delim_at + offset;
            let (first, rest) = (&trimmed[..at], &trimmed[at + 1..]);
            return Some((first.trim_end(), rest.trim_start()));
        }
    }
    None
}

/// Whether `|` belongs to this command's payload instead of separating Ex
/// commands. Shell/terminal commands own their complete tail; pattern commands
/// may contain a bar in their pattern.
///
/// We strip leading range characters (digits, commas, %, ., $, ', <, >) before
/// recognizing filter/read/write shell forms and pattern commands. This avoids
/// false positives on commands like `:e files/foo | set number`.
fn command_owns_bar(command: &str) -> bool {
    let trimmed = command.trim();
    if let Ok(parsed) = crate::commands::parse(trimmed) {
        if parsed.next.is_none() {
            return true;
        }
    }
    // Skip past range prefix: digits, commas, %, ., $, ', <, >, +, -, spaces
    let cmd = trimmed.trim_start_matches(|c: char| c.is_ascii_digit() || ",%.$ '<>+-".contains(c));
    cmd.starts_with('!')
        || ["r !", "read !", "w !", "write !"]
            .iter()
            .any(|prefix| cmd.starts_with(prefix))
        || cmd.starts_with("s/")
        || cmd.starts_with("g/")
        || cmd.starts_with("g!/")
        || cmd.starts_with("v/")
}

/// Execute a single command (no chaining)
fn execute_command_single(editor: &mut Editor, command: &str) -> Result<()> {
    // Update the : register with the command
    editor.registers_mut().set_last_command(command.to_string());

    let response = crate::commands::execute_command(editor, command);
    match response {
        CommandResult::Success(success_resp) => {
            // Command executed successfully
            if let Some(msg) = success_resp.message {
                // Multi-line messages go to hover popup, single-line to status bar
                let msg = msg.into_owned();
                if msg.contains('\n') {
                    editor.set_hover_info(msg);
                } else {
                    editor.set_status_message(msg);
                }
            }
            return Ok(());
        }
        CommandResult::Error(err_resp) => {
            // Check if it's an "unknown command" error
            if err_resp.error.contains("Not an editor command") {
                // Fall through to custom input-specific command handling below
            } else {
                // It's a real error from a known command
                editor.set_status_message(err_resp.error);
                return Ok(());
            }
        }
    }

    // Check if it's a :b <n> or :buffer <n> command
    if let Some(buffer_num_str) = command
        .strip_prefix("b ")
        .or_else(|| command.strip_prefix("buffer "))
    {
        if let Ok(buffer_num) = buffer_num_str.trim().parse::<usize>() {
            if buffer_num > 0 {
                // Convert from 1-indexed to 0-indexed
                editor.switch_to_buffer(buffer_num - 1);
            }
        }
        return Ok(());
    }

    // Check if it's a :colorscheme <name> or :colo <name> command
    if let Some(scheme_name) = command
        .strip_prefix("colorscheme ")
        .or_else(|| command.strip_prefix("colo "))
    {
        match editor.set_color_scheme(scheme_name.trim()) {
            Ok(_) => {
                let message = format!("Color scheme set to '{}'", scheme_name.trim());
                editor.set_status_message(message);
            }
            Err(e) => {
                let available = editor.list_color_schemes().join(", ");
                let message = format!("{}. Available schemes: {}", e, available);
                editor.set_status_message(message);
            }
        }
    } else if editor.status_message().is_empty() {
        // Truly unrecognized ex-command (no handler set a status). Report it
        // the way Vim does (E492) so the user gets feedback on typos instead
        // of silence, and so the headless API can surface it as an error.
        editor.set_status_message(format!("E492: Not an editor command: {}", command));
    }

    Ok(())
}
