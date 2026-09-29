//! Run / debug launch flow, end to end through the shared frontend tick.
//!
//! A scripted stdio language server plays Hyperion (`hyperion.resolveLaunch`),
//! a scripted stdio debug adapter plays `hyperion-lsp dap`, and the "JDK" is a
//! shell script, so these tests need no Java and no Hyperion binary.

mod helpers;

use helpers::EditorTest;
use ovim::frontend::{process_editor_tick, FrontendChannels};
use ovim::mode::Mode;
use ovim_core::language_catalog::{DynamicLanguageSpec, DynamicLspSpec, RegistrationOwner};
use ovim_core::launch::{LineKind, RunOutcome, RunStatus};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// `JAVA_HOME` is process-global; tests that fake the JDK take turns.
static JDK_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Session {
    test: EditorTest,
    channels: FrontendChannels,
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Session {
    /// A ready language server that advertises `commands`.
    async fn new(commands: &[&str]) -> Self {
        Self::with_capabilities(commands, json!({})).await
    }

    async fn with_capabilities(commands: &[&str], extra: Value) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let script = root.join("server.py");
        std::fs::write(&script, include_str!("helpers/controlled_lsp.py")).unwrap();
        std::fs::write(root.join("project.marker"), "").unwrap();
        std::fs::write(root.join("initialize-response.json"), {
            let mut capabilities = json!({
                "textDocumentSync": 1,
                "executeCommandProvider": {"commands": commands}
            });
            for (key, value) in extra.as_object().unwrap() {
                capabilities[key] = value.clone();
            }
            json!({"result": {"capabilities": capabilities}}).to_string()
        })
        .unwrap();
        let mut test = EditorTest::new("class Main {}\n");
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
        std::fs::write(root.join("Main.controlled"), "class Main {}\n").unwrap();
        test.set_file_path(root.join("Main.controlled").display().to_string());
        test.editor.request_lsp_init();
        let mut session = Self {
            test,
            channels: FrontendChannels::new(),
            _dir: dir,
            root,
        };
        session.wait_for_event("textDocument/didOpen").await;
        session
    }

    async fn tick(&mut self) {
        tokio::time::timeout(
            Duration::from_secs(1),
            process_editor_tick(&mut self.test.editor, &mut self.channels),
        )
        .await
        .expect("the launch flow blocked the input/render tick");
    }

    fn events_in(&self, file: &str, key: &str, name: &str) -> Vec<Value> {
        std::fs::read_to_string(self.root.join(file))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|event| event[key] == name)
            .collect()
    }

    fn lsp_events(&self, method: &str) -> Vec<Value> {
        self.events_in("events.jsonl", "method", method)
    }

    /// Every message the server received, requests and responses alike.
    fn lsp_events_raw(&self) -> Vec<Value> {
        std::fs::read_to_string(self.root.join("events.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .collect()
    }

    async fn wait_for_event(&mut self, method: &str) {
        self.until(&format!("LSP event {method}"), |s| {
            !s.lsp_events(method).is_empty()
        })
        .await;
    }

    /// Ticks until `done`, failing (with the console text) after 10 seconds.
    async fn until(&mut self, what: &str, done: impl Fn(&mut Session) -> bool) {
        let result = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                self.tick().await;
                if done(self) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        if result.is_err() {
            panic!(
                "timed out waiting for {what}\nstatus: {:?}\nconsole:\n{}",
                self.test.editor.status_message(),
                self.console_text()
            );
        }
    }

    fn script_resolve(&self, result: Value) {
        std::fs::write(
            self.root.join("response-workspace_executeCommand.json"),
            json!({"result": result}).to_string(),
        )
        .unwrap();
    }

    fn console_text(&self) -> String {
        self.test
            .editor
            .run_console()
            .viewed()
            .map(|run| {
                run.lines
                    .iter()
                    .map(|l| format!("[{:?}] {}", l.kind, l.text))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default()
    }

    fn run_finished(&self) -> bool {
        matches!(
            self.test.editor.run_console().viewed().map(|r| &r.status),
            Some(RunStatus::Done(_))
        )
    }

    fn outcome(&self) -> RunOutcome {
        match &self.test.editor.run_console().viewed().unwrap().status {
            RunStatus::Done(outcome) => outcome.clone(),
            other => panic!("run still active: {other:?}"),
        }
    }

    fn write_script(&self, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = self.root.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// Installs a fake `$JAVA_HOME/bin/java` running `body`.
    fn fake_java(&self, body: &str) {
        std::fs::create_dir_all(self.root.join("jdk/bin")).unwrap();
        self.write_script("jdk/bin/java", body);
        // SAFETY: serialized by JDK_LOCK; no other thread reads the variable.
        unsafe { std::env::set_var("JAVA_HOME", self.root.join("jdk")) };
    }

    fn main_plan(&self, build: Option<Value>) -> Value {
        json!({
            "name": "Main (app)", "kind": "main", "language": "java",
            "projectRoot": self.root, "moduleDir": self.root, "buildTool": "none",
            "build": build,
            "launch": {
                "mainClass": "com.example.Main", "classpath": "/cp/classes",
                "args": ["one", "two words"], "jvmArgs": ["-Xmx8m"],
                "cwd": self.root, "projectRoot": self.root, "env": {"GREETING": "hi"}
            },
            "warnings": []
        })
    }

    async fn stop_lsp(&self) {
        let _ = self
            .test
            .editor
            .lsp_manager()
            .unwrap()
            .stop_server("controlled")
            .await;
    }
}

fn resolve_commands() -> [&'static str; 2] {
    ["hyperion.resolveLaunch", "hyperion.runConfigurations"]
}

fn process_alive(pid: i64) -> bool {
    // SAFETY: probing for existence only.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

// ---------------------------------------------------------------------------
// Run (no debugger)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_at_cursor_resolves_via_the_server_and_streams_output_into_a_persistent_console() {
    let _jdk = JDK_LOCK.lock().await;
    let mut s = Session::new(&resolve_commands()).await;
    s.fake_java(
        "echo \"args: $@\" > \"$JAVA_HOME/../java-args.txt\"\n\
         echo \"greeting=$GREETING cwd=$(pwd)\"\n\
         echo 'to stdout'\n\
         echo 'to stderr' >&2\n\
         exit 3",
    );
    s.script_resolve(s.main_plan(None));

    s.test.keys(" rr");
    s.until("the run to finish", |s| s.run_finished()).await;

    let resolve = s.lsp_events("workspace/executeCommand");
    assert_eq!(resolve.len(), 1);
    assert_eq!(resolve[0]["params"]["command"], "hyperion.resolveLaunch");
    let args = &resolve[0]["params"]["arguments"][0];
    assert_eq!(args["target"], "auto");
    assert!(args["uri"].as_str().unwrap().ends_with("/Main.controlled"));
    assert_eq!(args["position"]["line"], 0);

    let java_args = std::fs::read_to_string(s.root.join("java-args.txt")).unwrap();
    assert_eq!(
        java_args.trim(),
        "args: -Xmx8m -cp /cp/classes com.example.Main one two words"
    );

    let run = s.test.editor.run_console().viewed().unwrap();
    let kinds: Vec<(LineKind, &str)> = run
        .lines
        .iter()
        .map(|l| (l.kind, l.text.as_str()))
        .collect();
    assert!(
        kinds.contains(&(LineKind::Stdout, "to stdout")),
        "{kinds:?}"
    );
    assert!(
        kinds.contains(&(LineKind::Stderr, "to stderr")),
        "{kinds:?}"
    );
    assert!(
        run.lines
            .iter()
            .any(|l| l.text.contains("greeting=hi") && l.text.contains(s.root.to_str().unwrap())),
        "env and cwd from the plan must reach the process: {kinds:?}"
    );
    assert_eq!(run.exit_code, Some(3));
    assert_eq!(s.outcome(), RunOutcome::Failed);
    assert!(run.duration.is_some());
    assert!(
        run.status_text().starts_with("failed (exit 3) in "),
        "{}",
        run.status_text()
    );

    // The console outlives the process.
    s.tick().await;
    assert!(s.test.editor.run_console().open);
    assert_eq!(
        s.test.editor.run_console().viewed().unwrap().lines.len(),
        run_len(&s)
    );
    s.stop_lsp().await;
}

fn run_len(s: &Session) -> usize {
    s.test.editor.run_console().viewed().unwrap().lines.len()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn build_runs_first_without_blocking_the_tick_and_its_failure_aborts_the_launch() {
    let _jdk = JDK_LOCK.lock().await;
    let mut s = Session::new(&resolve_commands()).await;
    s.fake_java("touch \"$JAVA_HOME/../java-ran\"");
    let source = s.root.join("src/A.java");
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    std::fs::write(&source, "class A {\n    int x\n}\n").unwrap();
    let build = s.write_script(
        "build.sh",
        &format!(
            "sleep 1\n\
             echo '> Task :compileJava FAILED'\n\
             echo '{path}:2: error: cannot find symbol' >&2\n\
             sleep 0.2\n\
             echo 'noise on stdout between the diagnostic lines'\n\
             sleep 0.2\n\
             echo '    int x' >&2\n\
             echo '        ^' >&2\n\
             echo '  symbol:   class Foo' >&2\n\
             echo '  location: class A' >&2\n\
             echo '1 error' >&2\n\
             exit 1",
            path = source.display()
        ),
    );
    s.script_resolve(s.main_plan(Some(json!({"argv": [build], "cwd": s.root}))));

    s.test.keys(" rr");
    // The build is slow: the tick must stay responsive while it runs.
    let started = std::time::Instant::now();
    s.until("the build to start", |s| {
        matches!(
            s.test.editor.run_console().viewed().map(|r| &r.status),
            Some(RunStatus::Active(ovim_core::launch::RunPhase::Building))
        )
    })
    .await;
    assert!(
        started.elapsed() < Duration::from_millis(900),
        "build must not be awaited in the tick"
    );

    s.until("the build to finish", |s| s.run_finished()).await;
    assert_eq!(s.outcome(), RunOutcome::BuildFailed);
    assert!(
        !s.root.join("java-ran").exists(),
        "a failed build must abort the launch"
    );

    let qf = s.test.editor.quickfix_list();
    assert_eq!(qf.len(), 1, "{:?}", qf.entries());
    let entry = &qf.entries()[0];
    assert_eq!(entry.filename.as_deref(), Some(source.as_path()));
    assert_eq!(
        (entry.lnum, entry.col),
        (2, 9),
        "column comes from the caret line\nentries={:?}\nruns={} status={:?}\n{}",
        qf.entries(),
        s.test.editor.run_console().runs.len(),
        s.test
            .editor
            .run_console()
            .viewed()
            .map(|r| r.status.clone()),
        s.console_text()
    );
    assert!(entry.text.contains("symbol: class Foo"), "{}", entry.text);
    assert!(s.test.editor.is_quickfix_window_open());
    assert!(
        s.test.editor.status_message().contains("Build failed"),
        "{}",
        s.test.editor.status_message()
    );
    assert!(s.test.editor.status_message().contains("Launch aborted"));
    s.stop_lsp().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_successful_build_is_followed_by_the_launch_and_the_edit_is_saved_first() {
    let _jdk = JDK_LOCK.lock().await;
    let mut s = Session::new(&resolve_commands()).await;
    s.fake_java("test -f \"$JAVA_HOME/../built\" && echo build-was-first");
    let build = s.write_script(
        "build.sh",
        &format!(
            "cp {main} \"{root}/built\"; echo building",
            main = s.root.join("Main.controlled").display(),
            root = s.root.display()
        ),
    );
    s.script_resolve(s.main_plan(Some(json!({"argv": [build], "cwd": s.root}))));

    // An unsaved edit must reach the build.
    s.test.keys("A // edited<Esc>");
    s.test.keys(" rr");
    s.until("the run to finish", |s| s.run_finished()).await;

    assert_eq!(s.outcome(), RunOutcome::Succeeded);
    assert!(
        s.console_text().contains("build-was-first"),
        "{}",
        s.console_text()
    );
    let built = std::fs::read_to_string(s.root.join("built")).unwrap();
    assert!(
        built.contains("// edited"),
        "unsaved edits are saved before the build: {built}"
    );
    s.stop_lsp().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_kills_the_program_and_everything_it_started() {
    let _jdk = JDK_LOCK.lock().await;
    let mut s = Session::new(&resolve_commands()).await;
    // java "starts" a helper that would outlive it.
    s.fake_java("sleep 300 &\necho $! > \"$JAVA_HOME/../helper.pid\"\necho started\nwait");
    s.script_resolve(s.main_plan(None));

    s.test.keys(" rr");
    s.until("the program to start", |s| {
        s.console_text().contains("started")
    })
    .await;
    let helper: i64 = std::fs::read_to_string(s.root.join("helper.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(process_alive(helper));

    s.test.keys(" rs");
    s.until("the run to stop", |s| s.run_finished()).await;
    assert_eq!(s.outcome(), RunOutcome::Stopped);
    for _ in 0..100 {
        if !process_alive(helper) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        !process_alive(helper),
        "Stop must kill the whole process group"
    );
    assert!(!s.test.editor.is_launch_active());
    s.stop_lsp().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rerun_replaces_the_running_program() {
    let _jdk = JDK_LOCK.lock().await;
    let mut s = Session::new(&resolve_commands()).await;
    s.fake_java("echo \"run $$\" >> \"$JAVA_HOME/../runs.txt\"\nsleep 300");
    s.script_resolve(s.main_plan(None));

    s.test.keys(" rr");
    s.until("first run", |s| s.root.join("runs.txt").exists())
        .await;
    s.test.keys(" rl");
    s.until("second run", |s| {
        std::fs::read_to_string(s.root.join("runs.txt"))
            .map(|t| t.lines().count() == 2)
            .unwrap_or(false)
    })
    .await;
    assert_eq!(s.test.editor.run_console().runs.len(), 2);
    assert_eq!(
        s.test.editor.run_console().runs[0].status,
        RunStatus::Done(RunOutcome::Stopped),
        "the replaced run is kept, marked stopped"
    );
    s.test.keys(" rs");
    s.until("stop", |s| s.run_finished()).await;
    s.stop_lsp().await;
}

// ---------------------------------------------------------------------------
// Resolution fallbacks and messages
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn with_nothing_runnable_and_no_config_the_message_says_what_to_do() {
    let mut s = Session::new(&resolve_commands()).await;
    s.script_resolve(Value::Null);

    s.test.press_key(ovim_core::KeyCode::F(5));
    s.until("the launch to give up", |s| s.run_finished()).await;
    let status = s.test.editor.status_message().to_string();
    assert!(status.contains("Nothing to debug here"), "{status}");
    assert!(status.contains(".ovim/debug.toml"), "actionable: {status}");
    assert!(!status.contains("configurationDone"), "{status}");
    assert!(!s.test.editor.is_debug_active());
    s.stop_lsp().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_server_without_resolve_launch_falls_back_to_debug_toml_and_says_so() {
    let _jdk = JDK_LOCK.lock().await;
    let mut s = Session::new(&["something.else"]).await;
    s.fake_java("echo from-config");
    std::fs::create_dir_all(s.root.join(".ovim")).unwrap();
    std::fs::write(
        s.root.join(".ovim/debug.toml"),
        "[[config]]\nname = \"App\"\ntype = \"launch\"\nmain_class = \"a.B\"\nclasspath = \"out\"\n",
    )
    .unwrap();

    s.test.keys(" rr");
    s.until("the run to finish", |s| s.run_finished()).await;
    assert_eq!(s.outcome(), RunOutcome::Succeeded);
    assert!(s.console_text().contains("from-config"));
    assert!(
        s.test.editor.lsp_manager().is_some()
            && s.lsp_events("workspace/executeCommand").is_empty(),
        "the server does not list the command, so it must not be sent"
    );
    s.stop_lsp().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn several_configs_open_a_picker_and_choosing_one_runs_it() {
    let _jdk = JDK_LOCK.lock().await;
    let mut s = Session::new(&["x"]).await;
    s.fake_java("echo \"ran $@\"");
    std::fs::create_dir_all(s.root.join(".ovim")).unwrap();
    std::fs::write(
        s.root.join(".ovim/debug.toml"),
        "[[config]]\nname = \"First\"\ntype = \"launch\"\nmain_class = \"a.First\"\n\n\
         [[config]]\nname = \"Second\"\ntype = \"launch\"\nmain_class = \"a.Second\"\n\n\
         [[config]]\nname = \"Attach\"\ntype = \"attach\"\nport = 1\n",
    )
    .unwrap();

    s.test.keys(" rc");
    s.until("the picker", |s| s.test.editor.mode() == Mode::Picker)
        .await;
    // Attach configs cannot be run without a debugger and are not offered.
    s.test.type_text("Second");
    s.tick().await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    s.tick().await;
    s.test.press_enter();
    s.until("the run", |s| s.run_finished()).await;
    assert!(
        s.console_text().contains("ran a.Second"),
        "{}",
        s.console_text()
    );
    s.stop_lsp().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_broken_debug_toml_is_reported_not_silently_ignored() {
    let mut s = Session::new(&["x"]).await;
    std::fs::create_dir_all(s.root.join(".ovim")).unwrap();
    std::fs::write(
        s.root.join(".ovim/debug.toml"),
        "[[config]]\nname = \"Half\"\ntype = \"launch\"\n",
    )
    .unwrap();
    s.test.keys(" rr");
    s.until("give up", |s| s.run_finished()).await;
    assert!(
        s.console_text().contains("Half") && s.console_text().contains("incomplete"),
        "{}",
        s.console_text()
    );
    s.stop_lsp().await;
}

// ---------------------------------------------------------------------------
// Debug
// ---------------------------------------------------------------------------

/// Both fakes log to `events.jsonl`; give the DAP one its own directory.
struct DebugSession {
    inner: Session,
    dap_dir: PathBuf,
}

impl DebugSession {
    async fn new(commands: &[&str]) -> Self {
        let inner = Session::new(commands).await;
        let dap_dir = inner.root.join("dap");
        std::fs::create_dir_all(&dap_dir).unwrap();
        Self { inner, dap_dir }
    }

    fn adapter(&self, scenario: Value) -> (String, Vec<String>) {
        let script = self.dap_dir.join("fake_dap.py");
        std::fs::write(&script, include_str!("helpers/fake_dap.py")).unwrap();
        std::fs::write(self.dap_dir.join("scenario.json"), scenario.to_string()).unwrap();
        (
            "python3".to_string(),
            vec![
                script.display().to_string(),
                self.dap_dir.display().to_string(),
            ],
        )
    }

    fn requests(&self, command: &str) -> Vec<Value> {
        dap_requests(&self.dap_dir, command)
    }

    fn pid(&self) -> Option<i64> {
        std::fs::read_to_string(self.dap_dir.join("events.jsonl"))
            .ok()?
            .lines()
            .find_map(|l| serde_json::from_str::<Value>(l).ok()?["adapterPid"].as_i64())
    }
}

fn dap_requests(dir: &Path, command: &str) -> Vec<Value> {
    std::fs::read_to_string(dir.join("events.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|e| e["command"] == command)
        .collect()
}

async fn wait_gone(pid: i64) {
    for _ in 0..200 {
        if !process_alive(pid) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn debug_at_cursor_launches_through_the_adapter_and_keeps_output_after_it_ends() {
    let mut d = DebugSession::new(&resolve_commands()).await;
    let adapter = d.adapter(json!({
        "on_configuration_done": [
            {"event": "output", "body": {"category": "stdout", "output": "hello\nwor"}},
            {"event": "output", "body": {"category": "stdout", "output": "ld\n"}},
            {"event": "output", "body": {"category": "stderr", "output": "boom\n"}},
            {"event": "exited", "body": {"exitCode": 2}, "delay": 0.2},
            {"event": "terminated"}
        ]
    }));
    d.inner.script_resolve(d.inner.main_plan(None));

    d.inner
        .test
        .editor
        .launch_at_cursor_with(ovim_core::launch::LaunchMode::Debug, Some(adapter));
    d.inner
        .until("the session to end", |s| s.run_finished())
        .await;

    let launch = d.requests("launch");
    assert_eq!(launch.len(), 1);
    let arguments = &launch[0]["arguments"];
    assert_eq!(arguments["mainClass"], "com.example.Main");
    assert_eq!(arguments["classpath"], "/cp/classes");
    assert_eq!(arguments["projectRoot"], d.inner.root.to_str().unwrap());
    assert_eq!(arguments["env"]["GREETING"], "hi");
    let order: Vec<String> = [
        "initialize",
        "launch",
        "setBreakpoints",
        "configurationDone",
    ]
    .iter()
    .filter(|c| !d.requests(c).is_empty())
    .map(|c| c.to_string())
    .collect();
    assert!(
        order.contains(&"initialize".to_string())
            && order.contains(&"configurationDone".to_string())
    );

    let run = d.inner.test.editor.run_console().viewed().unwrap();
    let out: Vec<(LineKind, &str)> = run
        .lines
        .iter()
        .map(|l| (l.kind, l.text.as_str()))
        .collect();
    assert!(out.contains(&(LineKind::Stdout, "hello")), "{out:?}");
    assert!(
        out.contains(&(LineKind::Stdout, "world")),
        "split chunks are joined: {out:?}"
    );
    assert!(out.contains(&(LineKind::Stderr, "boom")), "{out:?}");
    assert_eq!(run.exit_code, Some(2), "the exited event's code is shown");
    assert_eq!(d.inner.outcome(), RunOutcome::Failed);

    // Output survives the session; the adapter and UI state are gone.
    assert!(!d.inner.test.editor.is_debug_active());
    assert!(!d.inner.test.editor.debug_state().output_lines.is_empty());
    assert!(d.inner.test.editor.debug_state().execution_line.is_none());
    let pid = d.pid().unwrap();
    wait_gone(pid).await;
    assert!(
        !process_alive(pid),
        "the adapter process must not linger after terminate"
    );
    d.inner.stop_lsp().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f5_debug_start_and_space_dc_all_resolve_the_cursor_the_same_way() {
    let mut d = DebugSession::new(&resolve_commands()).await;
    d.inner.script_resolve(Value::Null);
    for keys in ["F5", " dc", " rd"] {
        if keys == "F5" {
            d.inner.test.press_key(ovim_core::KeyCode::F(5));
        } else {
            d.inner.test.keys(keys);
        }
        d.inner.until("give up", |s| s.run_finished()).await;
        assert!(d
            .inner
            .test
            .editor
            .status_message()
            .contains("Nothing to debug here"));
        d.inner.test.editor.clear_run_console();
    }
    d.inner.test.command("debug start");
    d.inner.until("give up", |s| s.run_finished()).await;
    let commands = d.inner.lsp_events("workspace/executeCommand");
    let resolves = commands
        .iter()
        .filter(|c| c["params"]["command"] == "hyperion.resolveLaunch")
        .count();
    assert_eq!(
        resolves, 4,
        "each entry point asks resolveLaunch once: {commands:?}"
    );
    assert!(
        !d.dap_dir.join("events.jsonl").exists(),
        "no adapter is started without a target"
    );
    d.inner.stop_lsp().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_launch_resets_everything_and_reports_the_reason() {
    let mut d = DebugSession::new(&resolve_commands()).await;
    let adapter =
        d.adapter(json!({"launch_error": "Could not find or load main class com.example.Main"}));
    d.inner.script_resolve(d.inner.main_plan(None));

    d.inner
        .test
        .editor
        .launch_at_cursor_with(ovim_core::launch::LaunchMode::Debug, Some(adapter));
    d.inner
        .until("the launch to fail", |s| s.run_finished())
        .await;

    assert!(
        matches!(d.inner.outcome(), RunOutcome::Error(m) if m.contains("Could not find or load main class"))
    );
    assert!(d
        .inner
        .test
        .editor
        .status_message()
        .contains("Could not find or load main class"));
    assert!(
        !d.inner.test.editor.is_debug_active(),
        "a failed launch must not leave the session active"
    );
    assert!(
        d.requests("configurationDone").is_empty(),
        "no configurationDone after a failed launch"
    );
    let pid = d.pid().unwrap();
    wait_gone(pid).await;
    assert!(
        !process_alive(pid),
        "the adapter is killed after a failed launch"
    );
    assert!(!d.inner.test.editor.is_launch_active());
    d.inner.stop_lsp().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_ends_a_live_debug_session_and_kills_an_adapter_that_ignores_disconnect() {
    let mut d = DebugSession::new(&resolve_commands()).await;
    let adapter = d.adapter(json!({"linger_after_disconnect": true}));
    d.inner.script_resolve(d.inner.main_plan(None));

    d.inner
        .test
        .editor
        .launch_at_cursor_with(ovim_core::launch::LaunchMode::Debug, Some(adapter));
    let dap_dir = d.dap_dir.clone();
    d.inner
        .until("configurationDone", |_| {
            !dap_requests(&dap_dir, "configurationDone").is_empty()
        })
        .await;
    d.inner
        .until("debugging phase", |s| {
            matches!(
                s.test.editor.run_console().viewed().map(|r| &r.status),
                Some(RunStatus::Active(ovim_core::launch::RunPhase::Debugging))
            ) && s.test.editor.is_debug_active()
        })
        .await;

    d.inner.test.keys(" ds");
    d.inner.until("stopped", |s| s.run_finished()).await;
    assert_eq!(d.inner.outcome(), RunOutcome::Stopped);
    assert!(!d.inner.test.editor.is_debug_active());
    let pid = d.pid().unwrap();
    wait_gone(pid).await;
    assert!(
        !process_alive(pid),
        "adapter must be killed on Stop even if it lingers"
    );
    d.inner.stop_lsp().await;
}

// ---------------------------------------------------------------------------
// Test debugging through the build tool
// ---------------------------------------------------------------------------

fn test_plan(s: &Session, debug_script: &Path) -> Value {
    json!({
        "name": "FooTest", "kind": "test", "language": "java",
        "projectRoot": s.root, "moduleDir": s.root, "buildTool": "gradle",
        "build": null,
        "launch": {"mainClass": "unused", "projectRoot": s.root},
        "test": {
            "className": "com.example.FooTest", "methodName": null,
            "gradleTask": ":test",
            "argv": [debug_script, "plain"],
            "debugArgv": [debug_script, "debug"],
            "cwd": s.root, "reportsDir": s.root.join("reports")
        }
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_debug_waits_for_the_listening_line_on_stdout_or_stderr_and_attaches_to_that_port() {
    for stream in ["stdout", "stderr"] {
        let mut d = DebugSession::new(&resolve_commands()).await;
        let adapter = d.adapter(json!({}));
        let redirect = if stream == "stderr" { " >&2" } else { "" };
        let script = d.inner.write_script(
            "gradlew-fake.sh",
            &format!(
                "echo 'Starting a Gradle Daemon'\nsleep 0.5\necho 'Listening for transport dt_socket at address: 41977'{redirect}\nsleep 300"
            ),
        );
        d.inner.script_resolve(test_plan(&d.inner, &script));

        d.inner
            .test
            .editor
            .launch_at_cursor_with(ovim_core::launch::LaunchMode::Debug, Some(adapter));
        let dap_dir = d.dap_dir.clone();
        d.inner
            .until("attach", |_| !dap_requests(&dap_dir, "attach").is_empty())
            .await;
        let attach = &d.requests("attach")[0]["arguments"];
        assert_eq!(
            attach["port"], 41977,
            "the port is parsed from the line ({stream}), not hardcoded"
        );
        assert_eq!(attach["host"], "127.0.0.1");
        d.inner
            .until("configurationDone", |_| {
                !dap_requests(&dap_dir, "configurationDone").is_empty()
            })
            .await;
        assert!(d.inner.console_text().contains("Starting a Gradle Daemon"));

        // Stop kills the debug-jvm child too.
        d.inner.test.keys(" rs");
        d.inner.until("stopped", |s| s.run_finished()).await;
        assert!(!d.inner.test.editor.is_debug_active());
        d.inner.stop_lsp().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_debug_that_never_listens_times_out_showing_the_output_and_kills_the_child() {
    let mut d = DebugSession::new(&resolve_commands()).await;
    let adapter = d.adapter(json!({}));
    let script = d.inner.write_script(
        "gradlew-fake.sh",
        "echo $$ > pid.txt\necho 'Downloading gradle-9.zip'\nsleep 300",
    );
    d.inner.script_resolve(test_plan(&d.inner, &script));
    d.inner
        .test
        .editor
        .set_debug_port_timeout(Duration::from_millis(800));

    d.inner
        .test
        .editor
        .launch_at_cursor_with(ovim_core::launch::LaunchMode::Debug, Some(adapter));
    d.inner.until("the timeout", |s| s.run_finished()).await;
    let status = d.inner.test.editor.status_message().to_string();
    assert!(status.contains("Timed out"), "{status}");
    assert!(
        status.contains("Downloading gradle-9.zip"),
        "the actual output is shown: {status}"
    );
    assert!(d.requests("attach").is_empty());
    let pid: i64 = std::fs::read_to_string(d.inner.root.join("pid.txt"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    wait_gone(pid).await;
    assert!(
        !process_alive(pid),
        "the suspended JVM launcher must be killed on failure"
    );
    d.inner.stop_lsp().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_run_reads_junit_reports_into_the_console_and_quickfix() {
    let mut s = Session::new(&resolve_commands()).await;
    std::fs::create_dir_all(s.root.join("reports")).unwrap();
    let source = s.root.join("src/test/java/com/example/FooTest.java");
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    std::fs::write(&source, "class FooTest {}\n").unwrap();
    let script = s.write_script(
        "gradlew-fake.sh",
        &format!(
            "cat > reports/TEST-com.example.FooTest.xml <<'EOF'\n\
             <testsuite name=\"com.example.FooTest\">\n\
             <testcase name=\"ok()\" classname=\"com.example.FooTest\" time=\"0.1\"/>\n\
             <testcase name=\"bad()\" classname=\"com.example.FooTest\" time=\"0.2\">\n\
             <failure message=\"expected 1\" type=\"AssertionError\">AssertionError\n\
             \tat com.example.FooTest.bad(FooTest.java:1)\n\
             </failure></testcase></testsuite>\n\
             EOF\n\
             echo '{}'\n\
             exit 1",
            "tests ran"
        ),
    );
    s.script_resolve(test_plan(&s, &script));
    s.test.keys(" rr");
    s.until("the tests to finish", |s| s.run_finished()).await;
    assert!(
        s.console_text().contains("Tests: 1 passed, 1 failed"),
        "{}",
        s.console_text()
    );
    assert!(s
        .console_text()
        .contains("FAILED com.example.FooTest.bad(): AssertionError: expected 1"));
    let entry = &s.test.editor.quickfix_list().entries()[0];
    assert_eq!(entry.filename.as_deref(), Some(source.as_path()));
    assert_eq!(entry.lnum, 1);
    s.stop_lsp().await;
}

// ---------------------------------------------------------------------------
// Console navigation
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_focus_scrolls_and_enter_jumps_to_a_stack_frame_in_the_project() {
    let _jdk = JDK_LOCK.lock().await;
    let mut s = Session::new(&resolve_commands()).await;
    let source = s.root.join("src/main/java/com/example/Main.java");
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    std::fs::write(
        &source,
        "package com.example;\nclass Main {\n  void boom() {}\n}\n",
    )
    .unwrap();
    s.fake_java(
        "echo 'Exception in thread \"main\" java.lang.IllegalStateException: no' >&2\n\
         echo '\tat com.example.Main.boom(Main.java:3)' >&2\n\
         echo '\tat java.base/java.lang.Thread.run(Thread.java:1583)' >&2\n\
         exit 1",
    );
    s.script_resolve(s.main_plan(None));
    s.test.keys(" rr");
    s.until("finish", |s| s.run_finished()).await;

    s.test.keys(" rf");
    assert_eq!(s.test.editor.mode(), Mode::RunConsole);
    let frame_line = s
        .test
        .editor
        .run_console()
        .viewed()
        .unwrap()
        .lines
        .iter()
        .position(|l| l.text.contains("Main.boom"))
        .unwrap();
    s.test.editor.run_console_mut().set_cursor(frame_line);
    s.test.press_enter();
    assert_eq!(s.test.editor.mode(), Mode::Normal);
    assert_eq!(
        s.test.editor.buffer().file_path().map(PathBuf::from),
        Some(source.canonicalize().unwrap())
    );
    assert_eq!(s.test.editor.buffer().cursor().line(), 2);

    // A JDK frame has no project source: say so instead of failing silently.
    s.test.keys(" rf");
    let jdk_line = s
        .test
        .editor
        .run_console()
        .viewed()
        .unwrap()
        .lines
        .iter()
        .position(|l| l.text.contains("Thread.run"))
        .unwrap();
    s.test.editor.run_console_mut().set_cursor(jdk_line);
    s.test.press_enter();
    assert!(
        s.test.editor.status_message().contains("Source not found"),
        "{}",
        s.test.editor.status_message()
    );

    // `x` clears finished runs, `q` leaves focus.
    s.test.keys("x");
    assert!(s.test.editor.run_console().runs.is_empty());
    s.test.keys("q");
    assert_eq!(s.test.editor.mode(), Mode::Normal);
    s.stop_lsp().await;
}

// ---------------------------------------------------------------------------
// Code lens
// ---------------------------------------------------------------------------

fn run_lens(line: u32) -> Value {
    json!({
        "range": {"start": {"line": line, "character": 4}, "end": {"line": line, "character": 8}},
        "command": {"title": "▶ Run", "command": "hyperion.run", "arguments": [{"className": "Main"}]}
    })
}

fn lens_events(s: &Session) -> usize {
    s.lsp_events("textDocument/codeLens").len()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn code_lenses_are_shown_refreshed_after_edits_and_on_server_request() {
    let mut s = Session::with_capabilities(
        &resolve_commands(),
        json!({"codeLensProvider": {"resolveProvider": false}}),
    )
    .await;
    std::fs::write(
        s.root.join("response-textDocument_codeLens.json"),
        json!({"result": [run_lens(0)]}).to_string(),
    )
    .unwrap();

    s.until("the lens to appear", |s| {
        !s.test.editor.code_lenses().is_empty()
    })
    .await;
    assert_eq!(lens_events(&s), 1);
    let eol = s.test.editor.decorations.eol_for_line(0);
    assert_eq!(eol.len(), 1);
    assert_eq!(eol[0].text, "  ▶ Run │ ▶ Debug");

    // Idle: no request storm.
    for _ in 0..30 {
        s.tick().await;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        lens_events(&s),
        1,
        "an unchanged buffer is not re-requested"
    );

    // An edit re-requests once it settles, and the new answer replaces the old.
    std::fs::write(
        s.root.join("response-textDocument_codeLens.json"),
        json!({"result": [run_lens(1)]}).to_string(),
    )
    .unwrap();
    s.test.keys("O// new first line<Esc>");
    s.until("the lens to move", |s| {
        s.test
            .editor
            .code_lenses()
            .first()
            .is_some_and(|l| l.line == 1)
    })
    .await;
    assert_eq!(lens_events(&s), 2);
    assert!(s.test.editor.decorations.eol_for_line(0).is_empty());
    assert_eq!(s.test.editor.decorations.eol_for_line(1).len(), 1);

    // workspace/codeLens/refresh makes the editor ask again without an edit.
    std::fs::write(
        s.root.join("push-after-textDocument_codeLens.json"),
        json!([{"id": 777, "method": "workspace/codeLens/refresh"}]).to_string(),
    )
    .unwrap();
    // The push fires after the *next* codeLens answer; provoke one with an edit.
    s.test.keys("A x<Esc>");
    s.until("the third request", |s| lens_events(s) >= 3).await;
    s.until("the refresh-triggered request", |s| lens_events(s) >= 4)
        .await;
    let refresh_reply = s
        .lsp_events_raw()
        .into_iter()
        .any(|e| e["id"] == 777 && e.get("method").is_none());
    assert!(refresh_reply, "the refresh request must be answered");
    s.stop_lsp().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn running_a_hyperion_run_lens_goes_through_resolve_launch_at_the_lens_not_the_servers_vm() {
    let _jdk = JDK_LOCK.lock().await;
    let mut s = Session::with_capabilities(
        &resolve_commands(),
        json!({"codeLensProvider": {"resolveProvider": false}}),
    )
    .await;
    s.test
        .set_buffer_content("class Main {\n  void main() {}\n}\n");
    std::fs::write(
        s.root.join("response-textDocument_codeLens.json"),
        json!({"result": [run_lens(1)]}).to_string(),
    )
    .unwrap();
    s.fake_java("echo real-jvm");
    s.script_resolve(s.main_plan(None));
    s.until("the lens", |s| !s.test.editor.code_lenses().is_empty())
        .await;

    // Not on the lens line: nothing to run.
    s.test.keys("gg");
    s.test.keys(" cl");
    assert_eq!(s.test.editor.status_message(), "No code lens on this line");

    s.test.keys("j");
    s.test.keys(" cl");
    s.until("the run", |s| s.run_finished()).await;
    let commands = s.lsp_events("workspace/executeCommand");
    assert!(
        commands
            .iter()
            .all(|c| c["params"]["command"] != "hyperion.run"),
        "the lens must not execute on the server: {commands:?}"
    );
    let resolve = commands
        .iter()
        .find(|c| c["params"]["command"] == "hyperion.resolveLaunch")
        .expect("resolveLaunch");
    let position = &resolve["params"]["arguments"][0]["position"];
    assert_eq!(
        (position["line"].as_u64(), position["character"].as_u64()),
        (Some(1), Some(4)),
        "resolved at the lens position"
    );
    assert!(s.console_text().contains("real-jvm"));
    s.stop_lsp().await;
}
