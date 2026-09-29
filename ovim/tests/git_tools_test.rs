//! Git from the editor: stage/unstage, commit and amend through the message
//! buffer, the status list opening diffs, line history and merge conflicts.

mod helpers;

use git2::{IndexAddOption, Repository, Signature};
use helpers::EditorTest;
use ovim_core::Mode;
use std::fs;
use std::path::PathBuf;

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    repo: Repository,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let repo = Repository::init(&root).unwrap();
        let mut config = repo.config().unwrap();
        config.set_str("user.name", "Test").unwrap();
        config.set_str("user.email", "t@example.com").unwrap();
        Self {
            _dir: dir,
            root,
            repo,
        }
    }

    fn write(&self, name: &str, content: &str) -> String {
        let path = self.root.join(name);
        fs::write(&path, content).unwrap();
        path.to_string_lossy().to_string()
    }

    fn commit_all(&self, message: &str) {
        let mut index = self.repo.index().unwrap();
        index
            .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();
        let tree = self.repo.find_tree(index.write_tree().unwrap()).unwrap();
        let signature = Signature::now("Test", "t@example.com").unwrap();
        let parent = self.repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        let parents: Vec<&git2::Commit> = parent.iter().collect();
        self.repo
            .commit(
                Some("HEAD"),
                &signature,
                &signature,
                message,
                &tree,
                &parents,
            )
            .unwrap();
    }

    fn head_message(&self) -> String {
        self.repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .message()
            .unwrap()
            .to_string()
    }

    fn staged(&self, name: &str) -> String {
        let mut index = self.repo.index().unwrap();
        index.read(true).unwrap();
        let entry = index.get_path(std::path::Path::new(name), 0).unwrap();
        String::from_utf8(self.repo.find_blob(entry.id).unwrap().content().to_vec()).unwrap()
    }
}

const TEN: &str = "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\nl10\n";

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn stage_hunk_commit_and_amend_through_the_leader_keys_and_the_message_buffer() {
    let fixture = Fixture::new();
    let file = fixture.write("a.txt", TEN);
    fixture.commit_all("init");
    fixture.write("a.txt", "l1\nL2\nl3\nl4\nl5\nl6\nl7\nl8\nL9\nl10\n");

    let mut test = EditorTest::new("");
    test.load_file(&file);
    // Stage only the change on line 9.
    test.keys("9G");
    test.keys(" gs");
    assert_eq!(
        fixture.staged("a.txt"),
        "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nL9\nl10\n"
    );

    // <Space>gc opens the message buffer in insert mode; write to commit.
    test.keys(" gc");
    test.assert_mode(Mode::Insert);
    assert!(test.editor.is_commit_message_buffer());
    test.type_text("Change line nine");
    test.press_esc();
    test.command("w");
    assert!(!test.editor.is_commit_message_buffer());
    assert_eq!(fixture.head_message(), "Change line nine");
    assert!(
        test.editor.buffer().file_path().unwrap().ends_with("a.txt"),
        "back in the file"
    );

    // <Space>gC amends: the previous message is the starting text.
    test.keys("2G");
    test.keys(" gS");
    test.keys(" gC");
    assert!(test
        .editor
        .buffer()
        .rope()
        .to_string()
        .starts_with("Change line nine"));
    test.keys("ggA, and two<Esc>");
    test.keys("ZZ");
    assert_eq!(fixture.head_message(), "Change line nine, and two");
    assert_eq!(
        fixture
            .repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .parent_count(),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn quitting_the_message_buffer_aborts_and_refuses_when_modified() {
    let fixture = Fixture::new();
    let file = fixture.write("a.txt", "one\n");
    fixture.commit_all("init");
    fixture.write("a.txt", "two\n");
    let mut test = EditorTest::new("");
    test.load_file(&file);
    test.command("GitStage");
    test.command("GitCommit");
    test.type_text("draft");
    test.press_esc();
    test.command("q");
    assert!(
        test.editor.is_commit_message_buffer(),
        ":q refuses with unsaved text"
    );
    test.command("q!");
    assert!(!test.editor.is_commit_message_buffer());
    assert_eq!(fixture.head_message(), "init");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn status_list_enter_opens_the_diff_review_on_that_file() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "one\n");
    fixture.write("b.txt", "one\n");
    fixture.commit_all("init");
    fixture.write("a.txt", "one changed\n");
    fixture.write("b.txt", "one also changed\n");
    let file = fixture.root.join("a.txt").to_string_lossy().to_string();

    let mut test = EditorTest::new("");
    test.load_file(&file);
    test.keys(" gg");
    test.assert_mode(Mode::Picker);
    // Second entry (b.txt), Enter.
    test.keys("<C-n>");
    test.press_enter();
    test.assert_mode(Mode::Normal);
    assert!(test.editor.is_diff_review_buffer(), "the review is showing");
    let line = test.editor.buffer().cursor().line();
    let header = test.editor.buffer().line_text(line).unwrap().to_string();
    assert!(
        header.contains("b.txt"),
        "cursor is on the b.txt section: {header:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn line_history_lists_the_commits_and_enter_shows_the_diff_of_one() {
    let fixture = Fixture::new();
    let file = fixture.write("a.txt", "alpha\nbeta\ngamma\n");
    fixture.commit_all("create");
    fixture.write("a.txt", "alpha\nbeta v2\ngamma\n");
    fixture.commit_all("edit beta");
    fixture.write("a.txt", "alpha\nbeta v2\ngamma\ndelta\n");
    fixture.commit_all("add delta");

    let mut test = EditorTest::new("");
    test.load_file(&file);
    test.keys("2G");
    test.keys(" gL");
    test.assert_mode(Mode::Picker);
    let picker = test.editor.picker().unwrap();
    assert_eq!(picker.title(), Some("History of line 2"));
    let rows: Vec<String> = picker
        .collect_filtered_results(10)
        .into_iter()
        .map(|r| r.display.clone())
        .collect();
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(rows[0].ends_with("Test  edit beta"), "{rows:?}");
    assert!(rows[1].ends_with("Test  create"), "{rows:?}");

    // Enter on the newest opens that commit's diff.
    test.press_enter();
    assert!(test.editor.is_diff_review_buffer());
    let text = test.editor.buffer().rope().to_string();
    assert!(text.contains("beta v2"), "{text}");
    assert!(
        !text.contains("delta"),
        "only that commit's changes: {text}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn file_history_and_root_commits_are_shown() {
    let fixture = Fixture::new();
    let file = fixture.write("a.txt", "one\n");
    fixture.commit_all("first");
    let mut test = EditorTest::new("");
    test.load_file(&file);
    test.command("GitLog");
    test.assert_mode(Mode::Picker);
    test.press_enter();
    // A root commit has no parent to review against: shown as a plain diff.
    assert!(test.editor.buffer().rope().to_string().contains("+one"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn conflict_markers_navigate_and_resolve_with_the_leader_keys() {
    let fixture = Fixture::new();
    let file = fixture.write(
        "c.txt",
        "top\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> topic\nmid\n<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> topic\nend\n",
    );
    let mut test = EditorTest::new("");
    test.load_file(&file);
    test.keys("]n");
    test.assert_cursor(1, 0);
    test.keys(" gmo");
    assert_eq!(
        test.buffer_content(),
        "top\nours\nmid\n<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> topic\nend\n"
    );
    test.keys("[n");
    test.assert_cursor(3, 0);
    test.keys(" gmt");
    assert_eq!(test.buffer_content(), "top\nours\nmid\ny\nend\n");
    test.keys("]n");
    assert!(test.editor.status_message().contains("No merge conflicts"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn git_commands_report_problems_instead_of_failing_silently() {
    let fixture = Fixture::new();
    let file = fixture.write("a.txt", "one\n");
    fixture.commit_all("init");
    let mut test = EditorTest::new("");
    test.load_file(&file);
    test.command("GitCommit");
    assert!(
        test.editor.status_message().contains("Nothing to commit"),
        "{}",
        test.editor.status_message()
    );
    test.command("GitStageHunk");
    assert!(
        test.editor.status_message().contains("No unstaged change"),
        "{}",
        test.editor.status_message()
    );
    test.command("GitStatus");
    assert!(
        test.editor.status_message().contains("clean"),
        "{}",
        test.editor.status_message()
    );

    let outside = tempfile::tempdir().unwrap();
    let plain = outside.path().join("x.txt");
    fs::write(&plain, "x\n").unwrap();
    let mut test = EditorTest::new("");
    test.load_file(&plain.to_string_lossy());
    test.command("GitStatus");
    assert!(
        test.editor.status_message().starts_with("Git:"),
        "{}",
        test.editor.status_message()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn status_list_keys_stage_unstage_and_edit_and_wq_commits() {
    let fixture = Fixture::new();
    let a = fixture.write("a.txt", "one\n");
    fixture.write("b.txt", "b\n");
    fixture.commit_all("init");
    fixture.write("a.txt", "two\n");
    let mut test = EditorTest::new("");
    test.load_file(&a);

    test.command("GitStatus");
    test.assert_mode(Mode::Picker);
    test.keys("<C-t>");
    assert_eq!(
        fixture.staged("a.txt"),
        "two\n",
        "Ctrl-T stages the selected file"
    );
    test.keys("<C-t>");
    assert_eq!(fixture.staged("a.txt"), "one\n", "and unstages it again");
    test.keys("<C-e>");
    test.assert_mode(Mode::Normal);
    assert!(test.editor.buffer().file_path().unwrap().ends_with("a.txt"));

    // :wq also commits from the message buffer.
    test.command("GitStage");
    test.command("GitCommit");
    test.type_text("Via wq");
    test.press_esc();
    test.command("wq");
    assert_eq!(fixture.head_message(), "Via wq");
}
