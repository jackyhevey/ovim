//! How shell-command runs (`<Space>t*` for non-JVM languages, `:make`) behave
//! end to end: streaming, finishing, the quickfix list they leave behind,
//! and what happens to the process when a run is superseded or stopped.
//!
//! These pin the behaviour of the old separate runners so the move onto the
//! launch pipeline's process handle changes only what is intended (see
//! OV-00480).

use crate::editor::{Editor, QuickfixEntryType, TestRunStatus};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Advances every background poller once.
fn poll_all(editor: &mut Editor) {
    editor.poll_pending_make();
    editor.poll_pending_test();
}

/// Polls until `done` holds (fails after 15 s).
async fn drive(editor: &mut Editor, what: &str, done: impl Fn(&Editor) -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        poll_all(editor);
        if done(editor) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn latest_status(editor: &Editor) -> Option<TestRunStatus> {
    editor.test_panel().latest().map(|run| run.status)
}

fn test_finished(editor: &Editor) -> bool {
    latest_status(editor).is_some_and(|s| s != TestRunStatus::Running)
}

fn make_finished(editor: &Editor) -> bool {
    editor.last_make_output().is_some()
}

#[cfg(unix)]
fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// A shell command that records the pid of the `sleep` it becomes.
fn sleeper(pid_file: &Path) -> String {
    format!("echo $$ > '{}'; exec sleep 60", pid_file.display())
}

async fn read_pid(pid_file: &Path) -> i32 {
    for _ in 0..500 {
        if let Ok(text) = std::fs::read_to_string(pid_file) {
            if let Ok(pid) = text.trim().parse() {
                return pid;
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the command never wrote {}", pid_file.display());
}

fn make(editor: &mut Editor, makeprg: &str) {
    editor.options.makeprg = makeprg.to_string();
    crate::commands::execute_command(editor, "make");
}

fn scratch() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().canonicalize().unwrap();
    (dir, path)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_test_run_streams_both_pipes_and_reports_success() {
    let (_dir, cwd) = scratch();
    let mut editor = Editor::with_content("");
    editor.spawn_test_job("suite", "echo to-out; echo to-err >&2", cwd);
    drive(&mut editor, "the run to finish", test_finished).await;
    let run = editor.test_panel().latest().unwrap();
    assert_eq!(run.status, TestRunStatus::Passed);
    assert!(run.lines.iter().any(|l| l == "to-out"), "{:?}", run.lines);
    assert!(run.lines.iter().any(|l| l == "to-err"), "{:?}", run.lines);
    assert!(editor.test_panel().open);
    assert_eq!(editor.last_make_output().unwrap().trim(), "to-out\nto-err");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failing_test_run_fills_quickfix_silently_with_paths_resolved_against_its_cwd() {
    let (_dir, cwd) = scratch();
    let mut editor = Editor::with_content("");
    let before = editor.buffer().file_path().map(str::to_string);
    editor.spawn_test_job(
        "file",
        "printf 'E   assert 1 == 2\\ntests/test_x.py:12: in test_x\\n'; exit 1",
        cwd.clone(),
    );
    drive(&mut editor, "the run to finish", test_finished).await;
    assert_eq!(latest_status(&editor), Some(TestRunStatus::Failed));
    let entries = editor.quickfix_list().entries();
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0].filename, Some(cwd.join("tests/test_x.py")));
    assert_eq!(entries[0].lnum, 12);
    assert!(editor.quickfix_list().title().starts_with("test "));
    // Silent: the panel is the visible surface, no jump and no window.
    assert!(!editor.is_quickfix_window_open());
    assert_eq!(editor.buffer().file_path().map(str::to_string), before);
}

#[tokio::test(flavor = "multi_thread")]
async fn make_lists_diagnostics_selects_the_first_entry_and_opens_the_window() {
    let (_dir, dir) = scratch();
    let warning = dir.join("a.rs");
    let error = dir.join("b.rs");
    std::fs::write(&warning, "fn a() {}\n").unwrap();
    std::fs::write(&error, "fn b() {}\n").unwrap();
    let mut editor = Editor::with_content("");
    make(
        &mut editor,
        &format!(
            "printf '{}:1:1: warning: w\\n{}:1:1: error: e\\n'; exit 1",
            warning.display(),
            error.display()
        ),
    );
    drive(&mut editor, "make to finish", make_finished).await;
    let list = editor.quickfix_list();
    assert_eq!(list.entries().len(), 2);
    assert!(list.title().starts_with(":make "), "{}", list.title());
    assert!(editor.is_quickfix_window_open());
    // Old behaviour: the jump goes to entry 0 even when it is a warning.
    assert_eq!(list.entries()[0].entry_type, QuickfixEntryType::Warning);
    assert_eq!(
        editor.buffer().file_path().map(PathBuf::from).as_deref(),
        Some(warning.as_path())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn make_leaves_paths_as_the_tool_printed_them() {
    let mut editor = Editor::with_content("");
    make(&mut editor, "printf 'rel/x.rs:3:1: error: boom\\n'; exit 1");
    drive(&mut editor, "make to finish", make_finished).await;
    // Old behaviour: no base directory, the relative name is kept as is.
    assert_eq!(
        editor.quickfix_list().entries()[0].filename,
        Some(PathBuf::from("rel/x.rs"))
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn superseding_a_test_run_leaves_the_old_process_running() {
    let (_dir, cwd) = scratch();
    let pid_file = cwd.join("pid");
    let mut editor = Editor::with_content("");
    editor.spawn_test_job("suite", &sleeper(&pid_file), cwd.clone());
    let pid = read_pid(&pid_file).await;
    editor.spawn_test_job("suite", "true", cwd);
    drive(&mut editor, "the second run to finish", test_finished).await;
    // Old behaviour (the bug OV-00480 fixes): nothing ever signals the first
    // run's process group.
    assert!(alive(pid));
    // SAFETY: cleaning up the process this test started.
    unsafe { libc::kill(pid, libc::SIGKILL) };
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_second_make_leaves_the_first_process_running() {
    let (_dir, cwd) = scratch();
    let pid_file = cwd.join("pid");
    let mut editor = Editor::with_content("");
    make(&mut editor, &sleeper(&pid_file));
    let pid = read_pid(&pid_file).await;
    make(&mut editor, "true");
    drive(&mut editor, "the second make to finish", make_finished).await;
    assert!(alive(pid));
    // SAFETY: cleaning up the process this test started.
    unsafe { libc::kill(pid, libc::SIGKILL) };
}
