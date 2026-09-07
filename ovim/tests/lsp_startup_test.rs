mod helpers;

use helpers::EditorTest;
use ovim::frontend::{process_editor_tick, FrontendChannels};
use ovim_core::language_catalog::{DynamicLanguageSpec, DynamicLspSpec, RegistrationOwner};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc;

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
        let script = dir.path().join("server.py");
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
                            dir.path().display().to_string(),
                        ],
                        language_id: "controlled".into(),
                        root_markers: vec!["project.marker".into()],
                    }),
                },
                RegistrationOwner::UserConfig {
                    source: dir.path().join("init.lua"),
                },
                &[dir.path().to_path_buf()],
            )
            .unwrap();
        std::fs::write(dir.path().join("project.marker"), "").unwrap();
        test.set_file_path(dir.path().join("first.controlled").display().to_string());
        test.editor.request_lsp_init();
        let (_, rx) = mpsc::channel(1);
        Self {
            test,
            channels: FrontendChannels::new(rx),
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
async fn file_switch_during_startup_waits_for_the_shared_server_and_opens_current_file() {
    let mut session = StartupSession::new();
    session.wait_for_event("initialize").await;
    let first = session
        .test
        .editor
        .buffer()
        .file_path()
        .unwrap()
        .to_string();
    let second: PathBuf = session.dir.path().join("second.controlled");

    std::fs::write(&second, "second file\n").unwrap();
    session.test.editor.new_tab();
    session.test.editor.open_file(&second).unwrap();
    session.tick().await;
    session.ready();

    let opened = session.wait_for_event("textDocument/didOpen").await;
    assert_eq!(
        opened["params"]["textDocument"]["uri"],
        ovim::lsp::uri_from_file_path(&second).unwrap().as_str()
    );
    assert_eq!(opened["params"]["textDocument"]["text"], "second file\n");
    assert_eq!(session.events("initialize").len(), 1);
    let manager = session.test.editor.lsp_manager().unwrap();
    assert_eq!(
        manager
            .get_document_version(&ovim::lsp::uri_from_file_path(first).unwrap())
            .await,
        0
    );
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
    let (_, rx) = mpsc::channel(1);
    session.channels = FrontendChannels::new(rx);
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
