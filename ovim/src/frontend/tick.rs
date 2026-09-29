use crate::buffer::{BufferId, LineHighlights};
use crate::editor::Editor;
use crate::mode::Mode;
use crate::syntax::{Language, LanguageRegistry, SyntaxHighlighter};

use super::channels::FrontendChannels;
use super::loading::{
    spawn_file_finder_loading, spawn_picker_preview_loading, update_file_list_cache_from_background,
};

/// Drives one round of background work: LSP, DAP, syntax highlighting,
/// picker, and installs. Call on a periodic interval from any frontend.
pub async fn process_editor_tick(editor: &mut Editor, channels: &mut FrontendChannels) {
    // Do not let a slow LSP initialization trap a yank flash on screen. Keep
    // deferring LSP while the flash is visible and for the tick that clears
    // it, giving the frontend one complete tick to paint the clear frame.
    let defer_lsp_for_yank_flash = process_yank_flash(editor);

    // Syntax must get a complete tick before LSP startup. Starting a language
    // server can take several seconds, and awaiting it first used to leave a
    // newly opened file unhighlighted for the entire startup window.
    let defer_lsp_init = process_syntax_highlighting(editor, channels) || defer_lsp_for_yank_flash;

    // === LSP lifecycle ===
    process_lsp_notifications(editor).await;
    channels.lsp_startup.poll(editor).await;
    if !defer_lsp_init {
        process_lsp_init(editor, channels);
    }
    process_lsp_sync_and_inlay_hints(editor).await;

    // === Debug adapter ===
    process_dap_events(editor);
    process_pending_debug_action(editor).await;
    // Build / run / debug launch state machine (non-blocking: child output
    // and language-server answers are polled, never awaited).
    if editor.poll_launch() {
        editor.mark_dirty();
    }
    // Code lenses: request once edits settle, apply when the server answers.
    editor.request_code_lens_if_needed().await;
    if editor.poll_code_lens() {
        editor.mark_dirty();
    }

    // === LSP responses & intents ===
    if editor.poll_pending_lsp_responses() {
        editor.mark_dirty();
    }
    editor.dispatch_pending_intents().await;

    // === Background tasks ===
    poll_background_tasks(editor).await;
    update_file_list_cache_from_background(editor, channels);

    // === Transient UI state ===
    tick_transient_ui(editor);

    // === Lua ===
    let _ = editor.process_lua_commands();

    // === LSP installs ===
    spawn_pending_installs(editor);
    if editor.poll_install_progress() {
        editor.mark_dirty();
    }

    // === Picker ===
    if editor.mode() == Mode::Picker {
        process_picker_tick(editor, channels);
    }

    // File switches queue didClose outside the async input dispatcher. Drive
    // that lifecycle from the shared tick so headless and TUI sessions agree.
    editor.send_lsp_close_if_needed().await;
}

/// Start or finish initial syntax work and report whether LSP initialization
/// should wait until a later tick. The extra tick lets the frontend paint the
/// completed syntax cache before a slow language-server startup is awaited.
fn process_syntax_highlighting(editor: &mut Editor, channels: &mut FrontendChannels) -> bool {
    let defer_lsp_init =
        editor.buffer().should_init_syntax() || editor.buffer().syntax_highlighting_is_loading();
    spawn_syntax_highlighting(editor, &channels.syntax_tx);
    drain_syntax_results(editor, &mut channels.syntax_rx);
    defer_lsp_init
}

/// Expire the yank flash without allowing slow LSP startup to delay the frame
/// that removes it. Returns true while LSP initialization should be deferred.
fn process_yank_flash(editor: &mut Editor) -> bool {
    let expired = editor.tick_yank_flash();
    if expired {
        editor.mark_dirty();
    }
    expired || editor.yank_flash().is_some()
}

fn tick_transient_ui(editor: &mut Editor) {
    if editor.tick_cat_animation()
        | editor.tick_toasts()
        | editor.tick_ai_chat_working_animation()
        | editor.tick_ai_chat_text_selection_autoscroll()
        | editor.poll_ai_subagent_repaint()
    {
        editor.mark_dirty();
    }
}

/// Process LSP notifications and server-initiated workspace edits.
async fn process_lsp_notifications(editor: &mut Editor) {
    if let Some(lsp_manager) = editor.lsp_manager() {
        let notification_count = lsp_manager.process_notifications().await;
        let flush_count = lsp_manager.process_flush_requests().await;

        if notification_count > 0 || flush_count > 0 {
            ovim_core::log_debug!(
                "tick",
                "LSP: {} notifications, {} flushes",
                notification_count,
                flush_count
            );
            editor.mark_dirty();
        }

        let pending_edits = lsp_manager.poll_pending_workspace_edits().await;
        for workspace_edit in pending_edits {
            ovim_core::log_debug!("tick", "Applying workspace edit from LSP server");
            match editor.apply_workspace_edit(workspace_edit) {
                Ok(applied) => {
                    if applied {
                        editor.set_lsp_status("Applied workspace edit".to_string());
                    } else {
                        editor.set_lsp_status("Partially applied workspace edit".to_string());
                    }
                }
                Err(e) => {
                    ovim_core::log_error!("tick", "Failed to apply workspace edit: {}", e);
                    editor.set_lsp_status(format!("Failed to apply edit: {}", e));
                }
            }
            editor.mark_dirty();
        }
    }
}

/// Initialize LSP for a newly opened file if needed.
fn process_lsp_init(editor: &mut Editor, channels: &mut FrontendChannels) {
    if let Some(approved) = editor.take_approved_lsp_install() {
        channels
            .lsp_startup
            .start(editor, &approved.file_path, true);
    }
    if let Some(file_path) = editor.needs_lsp_init() {
        ovim_core::log_debug!("tick", "Initializing LSP for {}", file_path);
        channels.lsp_startup.start(editor, &file_path, false);
        editor.clear_lsp_init_flag();
    }
}

/// Sync edits to the LSP server, refresh diagnostics, and poll inlay hints.
/// Colocated to enforce: server always has latest content before we check for fresh diagnostics.
async fn process_lsp_sync_and_inlay_hints(editor: &mut Editor) {
    if editor.sync_lsp_and_refresh_diagnostics().await {
        editor.mark_dirty();
    }
    if let Some(_lsp_manager) = editor.lsp_manager() {
        if editor.poll_pending_inlay_hint_response() {
            editor.mark_dirty();
        }
        if editor.inlay_hints_refresh_needed() {
            editor.request_inlay_hints_refresh().await;
        }
    }
}

/// Poll DAP events and auto-fetch stack trace on stop.
fn process_dap_events(editor: &mut Editor) {
    let dap_count = editor.process_dap_events();
    if dap_count > 0 {
        ovim_core::log_debug!("tick", "Processed {} DAP events", dap_count);
        editor.mark_dirty();
        if editor.debug_state().stopped_thread.is_some()
            && editor.debug_state().stack_frames.is_empty()
        {
            editor.dap_manager_mut().pending_action =
                Some(crate::dap::PendingDebugAction::FetchState);
        }
    }
}

/// Dispatch the pending debug action (start, stop, step, evaluate, etc.).
async fn process_pending_debug_action(editor: &mut Editor) {
    // Stop outranks everything else queued.
    if editor.dap_manager_mut().take_stop_request() {
        editor.dap_manager_mut().pending_action = None;
        if let Err(e) = editor.stop_debug_session().await {
            editor.set_status_message(format!("Debug stop failed: {e}"));
        }
        editor.mark_dirty();
        return;
    }
    if editor.dap_manager_mut().take_breakpoint_sync_request() && editor.is_debug_active() {
        let paths: Vec<std::path::PathBuf> =
            editor.debug_state().breakpoints.keys().cloned().collect();
        for path in &paths {
            let _ = editor.debug_sync_breakpoints(path).await;
        }
        let _ = editor.dap_manager().sync_exception_breakpoints().await;
        editor.mark_dirty();
    }
    let Some(action) = editor.dap_manager_mut().pending_action.take() else {
        return;
    };

    use crate::dap::PendingDebugAction;
    match action {
        PendingDebugAction::Start {
            command,
            args,
            launch,
        } => {
            if let Err(e) = editor.start_debug_session(&command, &args, launch).await {
                editor.launch_debug_failed(e.to_string());
            }
            editor.mark_dirty();
        }
        PendingDebugAction::Stop => {
            if let Err(e) = editor.stop_debug_session().await {
                editor.set_status_message(format!("Debug stop failed: {e}"));
            }
            editor.mark_dirty();
        }
        PendingDebugAction::Continue => {
            if let Err(e) = editor.debug_continue().await {
                editor.set_status_message(format!("Debug continue failed: {e}"));
            }
            editor.mark_dirty();
        }
        PendingDebugAction::StepOver => {
            if let Err(e) = editor.debug_step_over().await {
                editor.set_status_message(format!("Debug step failed: {e}"));
            }
            editor.mark_dirty();
        }
        PendingDebugAction::StepIn => {
            if let Err(e) = editor.debug_step_in().await {
                editor.set_status_message(format!("Debug step in failed: {e}"));
            }
            editor.mark_dirty();
        }
        PendingDebugAction::StepOut => {
            if let Err(e) = editor.debug_step_out().await {
                editor.set_status_message(format!("Debug step out failed: {e}"));
            }
            editor.mark_dirty();
        }
        PendingDebugAction::LaunchOrAttach => {
            process_dap_launch_or_attach(editor).await;
        }
        PendingDebugAction::SyncBreakpoints => {
            let paths: Vec<std::path::PathBuf> =
                editor.debug_state().breakpoints.keys().cloned().collect();
            for path in &paths {
                let _ = editor.debug_sync_breakpoints(path).await;
            }
            let _ = editor.dap_manager().sync_exception_breakpoints().await;
            match editor.dap_manager_mut().configuration_done().await {
                Ok(()) => editor.launch_debug_started(),
                Err(e) => {
                    let _ = editor.stop_debug_session().await;
                    editor.launch_debug_failed(format!("configurationDone failed: {e}"));
                }
            }
            editor.mark_dirty();
        }
        PendingDebugAction::RefreshWatches => {
            editor.debug_refresh_watches().await;
        }
        PendingDebugAction::EvaluateHover { expression } => {
            let frame_id = editor.selected_frame_id();
            match editor
                .dap_manager()
                .evaluate(&expression, frame_id, Some("hover"))
                .await
            {
                Ok((result, type_, var_ref)) => {
                    let mut text = format!("{expression} = {result}");
                    if let Some(type_) = type_.filter(|t| !t.is_empty()) {
                        text.push_str(&format!("\ntype: {type_}"));
                    }
                    if var_ref > 0 {
                        if let Ok(children) = editor.dap_manager().variables(var_ref).await {
                            text.push('\n');
                            for child in children.iter().take(20) {
                                text.push_str(&format!("\n  {} = {}", child.name, child.value));
                            }
                            if children.len() > 20 {
                                text.push_str(&format!("\n  ... {} more", children.len() - 20));
                            }
                        }
                    }
                    editor.set_hover_info(text);
                }
                Err(e) => editor.set_status_message(format!("{expression}: {e}")),
            }
            editor.mark_dirty();
        }
        PendingDebugAction::FetchState => {
            let _ = editor.debug_fetch_stack_trace().await;
            editor.debug_fetch_exception_info().await;
            editor.debug_fetch_threads().await;
            let _ = editor.debug_fetch_scopes().await;
            let scope_refs: Vec<u64> = editor
                .debug_state()
                .scopes
                .iter()
                .filter(|s| !s.expensive)
                .map(|s| s.variables_reference)
                .collect();
            for var_ref in scope_refs {
                let _ = editor.debug_fetch_variables(var_ref).await;
            }
            let expanded: Vec<u64> = editor.debug_state().expanded_refs.iter().copied().collect();
            for var_ref in expanded {
                let _ = editor.debug_fetch_variables(var_ref).await;
            }
            editor.debug_refresh_watches().await;
            editor.mark_dirty();
        }
        PendingDebugAction::SelectFrame { index: _ } => {
            let _ = editor.debug_fetch_scopes().await;
            let scope_refs: Vec<u64> = editor
                .debug_state()
                .scopes
                .iter()
                .filter(|s| !s.expensive)
                .map(|s| s.variables_reference)
                .collect();
            for var_ref in scope_refs {
                let _ = editor.debug_fetch_variables(var_ref).await;
            }
            editor.debug_refresh_watches().await;
            editor.mark_dirty();
        }
        PendingDebugAction::Evaluate { expression } => {
            let frame_id = editor.selected_frame_id();
            match editor
                .dap_manager()
                .evaluate(&expression, frame_id, Some("repl"))
                .await
            {
                Ok((result, _type, _var_ref)) => {
                    editor.set_status_message(format!("{expression} = {result}"));
                }
                Err(e) => {
                    editor.set_status_message(format!("Eval error: {e}"));
                }
            }
            editor.mark_dirty();
        }
        PendingDebugAction::FetchVariables { var_ref } => {
            let _ = editor.debug_fetch_variables(var_ref).await;
            editor.mark_dirty();
        }
    }
}

/// Send the DAP `launch` / `attach` request for the session being started.
///
/// Every way of starting a session (F5, `:debug start`, `<Space>dc`, the
/// config picker, test debugging) arrives here with a fully resolved request.
async fn process_dap_launch_or_attach(editor: &mut Editor) {
    use crate::dap::{DapLaunchRequest, PendingDebugAction};

    let Some(request) = editor.dap_manager().launch_request.clone() else {
        // Nothing was asked for; a stray `initialized` event. Do not send
        // `configurationDone` to an adapter that has no debuggee.
        return;
    };
    let result = match request {
        DapLaunchRequest::Launch(arguments) => editor.dap_manager_mut().launch(arguments).await,
        DapLaunchRequest::Attach(arguments) => editor.dap_manager_mut().attach(arguments).await,
    };
    match result {
        Ok(()) => {
            editor.dap_manager_mut().pending_action = Some(PendingDebugAction::SyncBreakpoints);
        }
        Err(e) => {
            let _ = editor.stop_debug_session().await;
            editor.launch_debug_failed(format!("launch/attach failed: {e}"));
        }
    }
    editor.mark_dirty();
}

/// Spawn background syntax highlighting if the buffer needs it.
fn spawn_syntax_highlighting(
    editor: &mut Editor,
    syntax_tx: &tokio::sync::mpsc::Sender<(BufferId, Language, Option<LineHighlights>, u64)>,
) {
    if !editor.buffer().should_init_syntax() {
        return;
    }
    let buf = editor.buffer();
    let buffer_id = buf.id();
    let source = buf.rope().to_string();
    let version = buf.highlight_version();
    if let Some(path) = buf.file_path() {
        if let Some(lang) = LanguageRegistry::detect_from_path(path) {
            editor.buffer_mut().mark_syntax_loading();
            let tx = syntax_tx.clone();
            tokio::task::spawn_blocking(move || {
                let highlights = if let Ok(mut h) = SyntaxHighlighter::new(lang) {
                    h.parse(&source);
                    Some(h.highlights_for_all_lines(&source))
                } else {
                    None
                };
                let _ = tx.blocking_send((buffer_id, lang, highlights, version));
            });
        } else if buf
            .language_catalog()
            .detect(path)
            .and_then(|language| language.syntax.clone())
            .is_some()
        {
            // Plugin parsers are already validated at startup. Keep the v1
            // handoff simple and initialize them on first display.
            editor.buffer_mut().enable_syntax_highlighting();
        }
    }
}

/// Drain completed background syntax results into buffers.
fn drain_syntax_results(
    editor: &mut Editor,
    syntax_rx: &mut tokio::sync::mpsc::Receiver<(BufferId, Language, Option<LineHighlights>, u64)>,
) {
    while let Ok((buffer_id, lang, highlights, version)) = syntax_rx.try_recv() {
        let is_current = editor.buffer().id() == buffer_id;
        if let Some(buffer) = editor.get_buffer_by_id_mut(buffer_id) {
            let applied = if let Some(highlights) = highlights {
                buffer.apply_background_syntax(lang, highlights, version)
            } else {
                buffer.clear_syntax_loading();
                false
            };

            if is_current && applied {
                editor.mark_dirty();
            }
        }
    }
}

/// Poll all independent background tasks (AI, make, git, chat, workflows).
async fn poll_background_tasks(editor: &mut Editor) {
    if let Some(url) = editor.take_pending_external_url() {
        let _ = open::that_in_background(&url);
    }
    if editor.poll_pending_codex_auth() {
        editor.mark_dirty();
    }
    if editor.poll_search_replace() {
        editor.mark_dirty();
    }
    editor.track_recent_file();
    editor.request_outline_if_needed().await;
    if editor.poll_outline() {
        editor.mark_dirty();
    }
    if editor.poll_git_refresh() {
        editor.mark_dirty();
    }
    if editor.poll_git_fetch() {
        editor.mark_dirty();
    }
    // The side-by-side diff review is laid out to a fixed width, so it has to
    // re-flow when the window changes size.
    if editor.relayout_diff_review() {
        editor.mark_dirty();
    }
    if editor.poll_pending_ai_chat_job() {
        editor.mark_dirty();
    }
    if editor.poll_pending_workflow_jobs() {
        editor.mark_dirty();
    }
}

/// Drive the picker: nucleo matching, grep drain, debounced filter, preview/file loading.
fn process_picker_tick(editor: &mut Editor, channels: &mut FrontendChannels) {
    let mut picker_changed = false;
    if let Some(picker) = editor.picker_mut() {
        if picker.tick() {
            picker_changed = true;
        }
        if picker.drain_grep_results() {
            picker_changed = true;
        }
    }
    if picker_changed {
        editor.mark_dirty();
    }
    if editor.apply_pending_picker_filter(50) {
        editor.mark_dirty();
    }
    spawn_picker_preview_loading(editor, &channels.preview_tx);
    spawn_file_finder_loading(editor, &channels.file_tx, &channels.file_list_cache_tx);
    if editor.picker_rapid_scrolling_just_stopped() {
        editor.mark_dirty();
    }
}

/// Spawn background tasks for pending LSP install requests
fn spawn_pending_installs(editor: &mut Editor) {
    use crate::editor::lsp_manager_panel::{InstallProgress, InstallStatus};

    let pending = editor.take_pending_installs();
    if pending.is_empty() {
        return;
    }

    let tx = editor.install_progress_tx().cloned();
    let Some(tx) = tx else { return };

    for request in pending {
        let tx = tx.clone();
        let lang_name = request.language_name.clone();
        let lang_id = request.language_id.clone();
        let config = request.auto_install_config.clone();
        let command = request.lsp_command.clone();

        tokio::spawn(async move {
            let _ = tx.send(InstallProgress {
                language_id: lang_id.clone(),
                status: InstallStatus::Installing(format!("Installing {lang_name}...")),
            });

            let result =
                crate::lsp_init::auto_install::attempt_auto_install(&lang_name, &command, &config)
                    .await;

            let status = match result {
                crate::lsp_init::auto_install::InstallResult::Success(_) => InstallStatus::Success,
                crate::lsp_init::auto_install::InstallResult::Failed(msg) => {
                    InstallStatus::Failed(msg)
                }
                crate::lsp_init::auto_install::InstallResult::PrerequisitesMissing(msg) => {
                    InstallStatus::Failed(msg)
                }
            };

            let _ = tx.send(InstallProgress {
                language_id: lang_id,
                status,
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{process_syntax_highlighting, process_yank_flash, tick_transient_ui};
    use crate::editor::Editor;
    use crate::frontend::FrontendChannels;
    use ovim_core::ai::chat_types::ChatOpts;

    #[test]
    fn working_animation_tick_invalidates_the_render_without_input() {
        let mut editor = Editor::with_content("hello\n");
        editor.open_ai_chat(ChatOpts::default()).unwrap();
        editor.ai_state.chat.as_mut().unwrap().waiting = true;
        editor.render_cache.ai_chat_working_animation_tick = u128::MAX;
        editor.mark_clean();

        tick_transient_ui(&mut editor);

        assert!(editor.is_dirty());
    }

    #[test]
    fn yank_flash_defers_slow_work_until_its_clear_frame_can_paint() {
        let mut editor = Editor::with_content("copy me\n");
        editor.set_yank_flash_lines(0, 0);

        assert!(process_yank_flash(&mut editor));
        assert!(editor.yank_flash().is_some());

        std::thread::sleep(std::time::Duration::from_millis(175));
        editor.mark_clean();

        assert!(process_yank_flash(&mut editor));
        assert!(editor.yank_flash().is_none());
        assert!(editor.is_dirty());
        assert!(
            !process_yank_flash(&mut editor),
            "slow work may start only after the clear frame has been deferred once"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn yaml_syntax_gets_a_paint_tick_before_lsp_initialization() {
        let mut editor = Editor::with_content("name: ovim\nenabled: true\n");
        editor.set_file_path("config.yaml".to_string());
        let mut channels = FrontendChannels::new();

        assert!(process_syntax_highlighting(&mut editor, &mut channels));

        for _ in 0..100 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            let defer_lsp = process_syntax_highlighting(&mut editor, &mut channels);
            if editor.buffer().has_syntax_highlighting() {
                assert!(
                    defer_lsp,
                    "the completion tick must still defer LSP so the frontend can paint"
                );
                assert!(!editor.buffer().highlights_for_line(0).is_empty());
                assert!(
                    !process_syntax_highlighting(&mut editor, &mut channels),
                    "LSP may initialize on the tick after syntax is ready"
                );
                return;
            }
        }

        panic!("YAML syntax highlighting did not finish");
    }
}
