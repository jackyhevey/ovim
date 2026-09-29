mod helpers;

use helpers::EditorTest;
use ovim::frontend::{process_editor_tick, FrontendChannels};
use ovim_core::language_catalog::{DynamicLanguageSpec, DynamicLspSpec, RegistrationOwner};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;

/// A real stdio peer holds initialize until the test releases it. Tests drive
/// the shared frontend tick, so a blocked startup fails before that release.
struct StartupSession {
    test: EditorTest,
    channels: FrontendChannels,
    dir: tempfile::TempDir,
}

impl StartupSession {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        // The editor canonicalizes paths it opens. On macOS the temp dir
        // sits behind the /var -> /private/var symlink, so every path the
        // harness hands the editor must already be canonical or the second
        // file lands in a "different" workspace root and starts a second
        // server.
        let root = dir.path().canonicalize().unwrap();
        let script = root.join("server.py");
        std::fs::write(&script, include_str!("helpers/controlled_lsp.py")).unwrap();
        let mut test = EditorTest::new("original\n");
        test.editor.enable_lsp();
        test.editor
            .language_catalog()
            .register_dynamic(
                DynamicLanguageSpec {
                    id: "controlled".into(),
                    name: "Controlled LSP".into(),
                    extensions: vec!["controlled".into()],
                    parser: None,
                    lsp: Some(DynamicLspSpec {
                        command: vec![
                            "python3".into(),
                            script.display().to_string(),
                            root.display().to_string(),
                        ],
                        language_id: "controlled".into(),
                        root_markers: vec!["project.marker".into()],
                    }),
                },
                RegistrationOwner::UserConfig {
                    source: root.join("init.lua"),
                },
                std::slice::from_ref(&root),
            )
            .unwrap();
        std::fs::write(root.join("project.marker"), "").unwrap();
        test.set_file_path(root.join("first.controlled").display().to_string());
        test.editor.request_lsp_init();
        Self {
            test,
            channels: FrontendChannels::new(),
            dir,
        }
    }

    async fn tick(&mut self) {
        tokio::time::timeout(
            Duration::from_secs(1),
            process_editor_tick(&mut self.test.editor, &mut self.channels),
        )
        .await
        .expect("LSP startup blocked the input/render tick");
    }

    fn events(&self, method: &str) -> Vec<Value> {
        std::fs::read_to_string(self.dir.path().join("events.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|event| event["method"] == method)
            .collect()
    }

    async fn wait_for_event(&mut self, method: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                self.tick().await;
                if let Some(event) = self.events(method).last() {
                    return event.clone();
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("server did not receive {method}"))
    }

    fn initialize_response(&self, payload: Value) {
        // Publish the complete response atomically; the peer may read immediately.
        let pending = self.dir.path().join("response.pending");
        std::fs::write(&pending, payload.to_string()).unwrap();
        std::fs::rename(pending, self.dir.path().join("initialize-response.json")).unwrap();
    }

    fn ready(&self) {
        self.initialize_response(json!({"result": {"capabilities": {"textDocumentSync": 1}}}));
    }

    async fn stop(&self) {
        self.test
            .editor
            .lsp_manager()
            .unwrap()
            .stop_server("controlled")
            .await
            .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn typing_during_startup_is_responsive_and_did_open_uses_latest_text() {
    let mut session = StartupSession::new();
    session.wait_for_event("initialize").await;

    session.test.keys("A edited<Esc>");
    session.tick().await;
    assert_eq!(session.test.buffer_content(), "original edited\n");
    assert!(session.events("textDocument/didOpen").is_empty());

    session.ready();
    let opened = session.wait_for_event("textDocument/didOpen").await;
    assert_eq!(
        opened["params"]["textDocument"]["text"],
        "original edited\n"
    );
    assert_eq!(session.events("initialize").len(), 1);
    session.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_open_buffer_is_opened_once_the_shared_server_is_ready() {
    let mut session = StartupSession::new();
    session.wait_for_event("initialize").await;
    let first = session
        .test
        .editor
        .buffer()
        .file_path()
        .unwrap()
        .to_string();
    let second: PathBuf = session
        .dir
        .path()
        .canonicalize()
        .unwrap()
        .join("second.controlled");

    std::fs::write(&second, "second file\n").unwrap();
    session.test.editor.new_tab();
    session.test.editor.open_file(&second).unwrap();
    session.tick().await;
    session.ready();

    // Both the current buffer and the buffer left behind in the first tab
    // must reach the server; only one server is started for the shared root.
    tokio::time::timeout(Duration::from_secs(5), async {
        while session.events("textDocument/didOpen").len() < 2 {
            session.tick().await;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("both open buffers must be didOpen'd");
    let opened = session.events("textDocument/didOpen");
    let text_for = |path: &str| {
        let uri = ovim::lsp::uri_from_file_path(path).unwrap();
        opened
            .iter()
            .find(|event| event["params"]["textDocument"]["uri"] == uri.as_str())
            .unwrap_or_else(|| panic!("no didOpen for {path}"))["params"]["textDocument"]["text"]
            .clone()
    };
    assert_eq!(text_for(&second.display().to_string()), "second file\n");
    assert_eq!(text_for(&first), "original\n");
    assert_eq!(session.events("initialize").len(), 1);
    session.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn buffer_opened_in_a_split_after_startup_is_opened_on_the_server() {
    let mut session = StartupSession::new();
    session.ready();
    session.wait_for_event("textDocument/didOpen").await;

    let second: PathBuf = session
        .dir
        .path()
        .canonicalize()
        .unwrap()
        .join("split.controlled");
    std::fs::write(&second, "in a split\n").unwrap();
    session.test.keys(":vsplit<CR>");
    session.test.keys(&format!(":e {}<CR>", second.display()));
    tokio::time::timeout(Duration::from_secs(5), async {
        while session.events("textDocument/didOpen").len() < 2 {
            session.tick().await;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("split buffer must be didOpen'd");
    session.test.keys("<C-w>w");
    session.tick().await;
    // Switching focus between windows must not close either document.
    for _ in 0..5 {
        session.tick().await;
    }
    assert!(session.events("textDocument/didClose").is_empty());
    session.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn startup_failure_is_reported_without_disabling_editing() {
    let mut session = StartupSession::new();
    session.wait_for_event("initialize").await;
    session.initialize_response(
        json!({"error": {"code": -32603, "message": "deliberate startup failure"}}),
    );

    tokio::time::timeout(Duration::from_secs(5), async {
        while !session
            .test
            .editor
            .lsp_status()
            .contains("deliberate startup failure")
        {
            session.tick().await;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("startup failure was not reported");
    session.test.keys("ccstill editable<Esc>");
    session.tick().await;
    assert_eq!(session.test.buffer_content(), "still editable\n");
    assert!(session.events("textDocument/didOpen").is_empty());
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_startup_reaps_its_process_and_can_be_retried() {
    use nix::sys::signal::kill;
    use nix::unistd::Pid;

    let mut session = StartupSession::new();
    let initialization = session.wait_for_event("initialize").await;
    let pid = Pid::from_raw(initialization["peerPid"].as_i64().unwrap() as i32);

    // Closing the frontend aborts its pending startup without a server response.
    session.channels = FrontendChannels::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        while kill(pid, None).is_ok() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("cancelled startup left its language server running");

    session.test.editor.request_lsp_init();
    session.ready();
    let opened = session.wait_for_event("textDocument/didOpen").await;
    assert_eq!(opened["params"]["textDocument"]["text"], "original\n");
    assert_eq!(session.events("initialize").len(), 2);
    session.stop().await;
}

#[cfg(unix)]
fn sigkill(pid: i64) {
    use nix::sys::signal::{kill, Signal};
    use nix::unistd::Pid;
    kill(Pid::from_raw(pid as i32), Signal::SIGKILL).unwrap();
}

#[cfg(unix)]
impl StartupSession {
    async fn wait_until(&mut self, what: &str, mut done: impl FnMut(&mut Self) -> bool) {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                self.tick().await;
                if done(self) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "timed out waiting for {what}; status: {}",
                self.test.editor.lsp_status()
            )
        });
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn killed_server_is_reaped_restarted_and_documents_reopened() {
    use nix::sys::signal::kill;
    use nix::unistd::Pid;

    let mut session = StartupSession::new();
    session.ready();
    let manager = session.test.editor.lsp_manager().unwrap();
    manager.set_restart_base_backoff(Duration::from_millis(20));
    let first_open = session.wait_for_event("textDocument/didOpen").await;
    let pid = session.events("initialize")[0]["peerPid"].as_i64().unwrap();

    sigkill(pid);
    session
        .wait_until("crash to be reported", |s| {
            s.test.editor.lsp_status().contains("crashed")
        })
        .await;
    let status = session.test.editor.lsp_status().to_string();
    assert!(
        status.contains("signal 9"),
        "status should say why: {status}"
    );
    // A zombie still answers signal 0; a reaped process does not.
    assert!(
        kill(Pid::from_raw(pid as i32), None).is_err(),
        "the killed server must be reaped, not left as a zombie"
    );

    session
        .wait_until("second didOpen after restart", |s| {
            s.events("textDocument/didOpen").len() >= 2
        })
        .await;
    assert_eq!(session.events("initialize").len(), 2);
    let reopened = session.events("textDocument/didOpen");
    assert_eq!(
        reopened[1]["params"]["textDocument"]["uri"],
        first_open["params"]["textDocument"]["uri"]
    );
    assert_ne!(
        reopened[1]["peerPid"].as_i64().unwrap(),
        pid,
        "didOpen must reach the replacement process"
    );

    // Explicit restart of a healthy server, through the ex command.
    session.test.keys(":LspRestart<CR>");
    session
        .wait_until("third didOpen after :LspRestart", |s| {
            s.events("textDocument/didOpen").len() >= 3
        })
        .await;
    assert_eq!(session.events("initialize").len(), 3);
    session.stop().await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_attempts_are_bounded_until_lsp_restart() {
    let mut session = StartupSession::new();
    session.ready();
    let manager = session.test.editor.lsp_manager().unwrap();
    manager.set_restart_base_backoff(Duration::from_millis(5));
    session.wait_for_event("textDocument/didOpen").await;
    let pid = session.events("initialize")[0]["peerPid"].as_i64().unwrap();

    // Every replacement fails its handshake.
    session.initialize_response(json!({"error": {"code": -32603, "message": "boom"}}));
    sigkill(pid);
    session
        .wait_until("giving up", |s| {
            s.test.editor.lsp_status().contains("giving up")
        })
        .await;
    let attempts = session.events("initialize").len();
    assert_eq!(attempts, 1 + ovim::lsp::MAX_AUTO_RESTARTS as usize);
    for _ in 0..30 {
        session.tick().await;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        session.events("initialize").len(),
        attempts,
        "no further automatic restarts after giving up"
    );

    // :LspRestart resets the budget.
    session.ready();
    session.test.keys(":LspRestart<CR>");
    session
        .wait_until("manual restart", |s| {
            s.events("initialize").len() > attempts
        })
        .await;
    session.stop().await;
}
