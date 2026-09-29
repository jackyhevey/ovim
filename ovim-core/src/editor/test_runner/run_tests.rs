//! How shell-command runs (`<Space>t*` for non-JVM languages, `:make`) behave
//! end to end: streaming, finishing, the quickfix list they leave behind,
//! and what happens to the process when a run is superseded or stopped.
//!
//! Both go through the launch pipeline's process handle (OV-00480). The
//! tests were written against the old separate runners first; the places
//! where the behaviour changed on purpose are marked "was:".

use crate::editor::{Editor, LaunchRequest, QuickfixEntryType, TestRunStatus};
use crate::launch::plan::PlanKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Advances every background poller once.
fn poll_all(editor: &mut Editor) {
    editor.poll_launch();
}

fn run_test(editor: &mut Editor, label: &'static str, command: &str, cwd: PathBuf) {
    editor.begin_request(LaunchRequest::shell(PlanKind::Test, label, command, cwd));
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
    run_test(&mut editor, "suite", "echo to-out; echo to-err >&2", cwd);
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
    run_test(
        &mut editor,
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
    // Was: the jump went to entry 0 even when that is a warning. Now it is
    // the first error, like a failed build in the launch pipeline.
    assert_eq!(list.entries()[0].entry_type, QuickfixEntryType::Warning);
    assert_eq!(list.selected_index(), 1);
    assert_eq!(
        editor.buffer().file_path().map(PathBuf::from).as_deref(),
        Some(error.as_path())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn make_resolves_relative_paths_against_the_directory_it_ran_in() {
    let mut editor = Editor::with_content("");
    make(&mut editor, "printf 'rel/x.rs:3:1: error: boom\\n'; exit 1");
    drive(&mut editor, "make to finish", make_finished).await;
    // Was: no base directory, the relative name was kept as printed.
    let cwd = std::env::current_dir().unwrap();
    assert_eq!(
        editor.quickfix_list().entries()[0].filename,
        Some(cwd.join("rel/x.rs"))
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn superseding_a_test_run_kills_the_old_process() {
    let (_dir, cwd) = scratch();
    let pid_file = cwd.join("pid");
    let mut editor = Editor::with_content("");
    run_test(&mut editor, "suite", &sleeper(&pid_file), cwd.clone());
    let pid = read_pid(&pid_file).await;
    run_test(&mut editor, "suite", "true", cwd);
    drive(&mut editor, "the second run to finish", test_finished).await;
    // Was: nothing ever signalled the first run's process group.
    assert!(!alive(pid));
    assert_eq!(editor.test_panel().runs[0].status, TestRunStatus::Cancelled);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_second_make_kills_the_first_process() {
    let (_dir, cwd) = scratch();
    let pid_file = cwd.join("pid");
    let mut editor = Editor::with_content("");
    make(&mut editor, &sleeper(&pid_file));
    let pid = read_pid(&pid_file).await;
    make(&mut editor, "true");
    drive(&mut editor, "the second make to finish", make_finished).await;
    assert!(!alive(pid));
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn stopping_kills_a_running_test_and_marks_it_cancelled() {
    let (_dir, cwd) = scratch();
    let pid_file = cwd.join("pid");
    let mut editor = Editor::with_content("");
    run_test(&mut editor, "suite", &sleeper(&pid_file), cwd);
    let pid = read_pid(&pid_file).await;
    assert!(editor.launch_stop());
    drive(&mut editor, "the run to stop", test_finished).await;
    assert_eq!(latest_status(&editor), Some(TestRunStatus::Cancelled));
    assert!(!alive(pid));
    assert!(!editor.is_launch_active());
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn stopping_kills_a_running_make() {
    let (_dir, cwd) = scratch();
    let pid_file = cwd.join("pid");
    let mut editor = Editor::with_content("");
    make(&mut editor, &sleeper(&pid_file));
    let pid = read_pid(&pid_file).await;
    assert!(editor.launch_stop());
    drive(&mut editor, "make to stop", |e| !e.is_launch_active()).await;
    assert!(!alive(pid));
    // A stopped make reports nothing.
    assert!(editor.last_make_output().is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_test_run_appears_in_the_run_console_and_rerun_replays_it() {
    let (_dir, cwd) = scratch();
    let mut editor = Editor::with_content("");
    run_test(&mut editor, "nearest", "echo hello", cwd.clone());
    drive(&mut editor, "the run to finish", test_finished).await;
    let console = editor.run_console().viewed().unwrap();
    assert_eq!(console.command, "echo hello");
    assert!(console.lines.iter().any(|l| l.text == "hello"));
    let first = editor.test_panel().runs.len();
    editor.launch_last();
    drive(&mut editor, "the rerun to start", |e| {
        e.test_panel().runs.len() > first
    })
    .await;
    drive(&mut editor, "the rerun to finish", test_finished).await;
    assert_eq!(latest_status(&editor), Some(TestRunStatus::Passed));
}
