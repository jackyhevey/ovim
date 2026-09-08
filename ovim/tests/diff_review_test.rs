//! Branch diff review (`<Space>gd`): open, navigate hunks, jump into files,
//! resume, refresh and close.

mod helpers;

use git2::{IndexAddOption, Oid, Repository, Signature};
use helpers::EditorTest;
use ovim_core::KeyCode;
use std::fs;
use std::path::Path;

fn commit_all(repo: &Repository, message: &str) -> Oid {
    let mut index = repo.index().unwrap();
    index
        .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
        .unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let signature = Signature::now("Ovim", "ovim@example.com").unwrap();
    let parent = repo.head().ok().and_then(|head| head.peel_to_commit().ok());
    let parents: Vec<&git2::Commit> = parent.iter().collect();
    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        message,
        &tree,
        &parents,
    )
    .unwrap()
}

/// A repo with `main` (a.txt = one/two/three) and a checked-out `feature`
/// branch that committed a change to a.txt and has an untracked b.txt.
struct Fixture {
    _dir: tempfile::TempDir,
    root: std::path::PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let repo = Repository::init(&root).unwrap();
        repo.set_head("refs/heads/main").unwrap();
        fs::write(root.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        commit_all(&repo, "c1");

        let head = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch("feature", &head, false).unwrap();
        repo.set_head("refs/heads/feature").unwrap();
        fs::write(root.join("a.txt"), "one\n2\nthree\nfour\n").unwrap();
        commit_all(&repo, "edit a");
        fs::write(root.join("b.txt"), "new file\n").unwrap();
        Self { _dir: dir, root }
    }

    fn path(&self, name: &str) -> String {
        self.root.join(name).to_string_lossy().to_string()
    }
}

fn open_editor_on(fixture: &Fixture, name: &str) -> EditorTest {
    let mut test = EditorTest::new("");
    test.editor
        .open_file(Path::new(&fixture.path(name)))
        .unwrap();
    test
}

fn current_line(test: &EditorTest) -> String {
    let line = test.editor.buffer().cursor().line();
    test.editor
        .buffer()
        .line_text(line)
        .map(|text| text.to_string())
        .unwrap_or_default()
}

fn line_index_of(test: &EditorTest, needle: &str) -> usize {
    let buffer = test.editor.buffer();
    (0..buffer.line_count())
        .find(|&index| buffer.line_text(index).as_deref() == Some(needle))
        .unwrap_or_else(|| panic!("no line {needle:?} in review"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn leader_gd_opens_a_highlighted_review_in_a_new_tab() {
    let fixture = Fixture::new();
    let mut test = open_editor_on(&fixture, "a.txt");
    assert_eq!(test.editor.tab_count(), 1);

    test.keys(" gd");

    assert!(test.editor.is_diff_review_buffer());
    assert_eq!(test.editor.tab_count(), 2);
    assert!(test.editor.buffer().is_read_only());
    assert_eq!(
        test.editor.buffer().display_name(),
        Some("Diff · feature → main")
    );
    assert_eq!(current_line(&test), "feature → main");

    let text = test.editor.buffer().rope().to_string();
    assert!(text.contains("2 files · +3 −1"), "{text}");
    assert!(text.contains("  M  a.txt  +2 −1"), "{text}");
    assert!(text.contains("  A  b.txt  +1"), "{text}");
    assert!(text.contains("@@ -1,3 +1,4 @@"), "{text}");
    assert!(text.contains("\n+new file\n"), "{text}");
    assert!(text.contains("1 commit ahead"), "{text}");

    // The pathless buffer still gets the diff grammar.
    assert!(
        test.editor.buffer().has_syntax_highlighting(),
        "review buffer should be highlighted as a diff"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn bracket_c_walks_hunks_and_enter_opens_the_source_line() {
    let fixture = Fixture::new();
    let mut test = open_editor_on(&fixture, "a.txt");
    test.keys(" gd");

    test.keys("]c");
    assert_eq!(current_line(&test), "@@ -1,3 +1,4 @@");
    test.keys("]c");
    assert_eq!(current_line(&test), "@@ -0,0 +1 @@");
    test.keys("]c");
    assert_eq!(
        current_line(&test),
        "@@ -0,0 +1 @@",
        "stays on the last hunk"
    );
    assert_eq!(test.editor.status_message(), "Last hunk");
    test.keys("[c");
    assert_eq!(current_line(&test), "@@ -1,3 +1,4 @@");

    // Land on the added `+four` line, column 3 → column 2 in the file.
    let four = line_index_of(&test, "+four");
    test.set_cursor(four, 3);
    test.press_key(KeyCode::Enter);

    assert!(!test.editor.is_diff_review_buffer());
    assert_eq!(
        test.editor.tab_count(),
        2,
        "file opens in the originating tab"
    );
    assert_eq!(test.editor.current_tab_index(), 0);
    assert!(test.editor.buffer().file_path().unwrap().ends_with("a.txt"));
    let cursor = test.editor.buffer().cursor();
    assert_eq!((cursor.line(), cursor.col().0), (3, 2));
    assert_eq!(current_line(&test), "four");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn enter_on_a_removed_line_lands_where_the_removal_happened() {
    let fixture = Fixture::new();
    let mut test = open_editor_on(&fixture, "a.txt");
    test.keys(" gd");

    let removed = line_index_of(&test, "-two");
    test.set_cursor(removed, 0);
    test.press_key(KeyCode::Enter);

    assert!(test.editor.buffer().file_path().unwrap().ends_with("a.txt"));
    assert_eq!(test.editor.buffer().cursor().line(), 1);
    assert_eq!(current_line(&test), "2");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn enter_on_a_file_row_jumps_to_that_file_and_opens_new_files() {
    let fixture = Fixture::new();
    let mut test = open_editor_on(&fixture, "a.txt");
    test.keys(" gd");

    let row = line_index_of(&test, "  A  b.txt  +1");
    test.set_cursor(row, 0);
    test.press_key(KeyCode::Enter);
    assert!(test.editor.is_diff_review_buffer());
    assert_eq!(current_line(&test), "diff --git a/b.txt b/b.txt");

    // Enter on a file header opens the file at its first hunk.
    test.press_key(KeyCode::Enter);
    assert!(test.editor.buffer().file_path().unwrap().ends_with("b.txt"));
    assert_eq!(test.editor.buffer().cursor().line(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn leader_gd_resumes_the_review_at_the_same_hunk_after_editing() {
    let fixture = Fixture::new();
    let mut test = open_editor_on(&fixture, "a.txt");
    test.keys(" gd");

    let four = line_index_of(&test, "+four");
    test.set_cursor(four, 0);
    test.press_key(KeyCode::Enter);
    assert_eq!(current_line(&test), "four");

    // Edit the file on disk (as if saved) so the refreshed review differs.
    fs::write(fixture.path("a.txt"), "zero\none\n2\nthree\nfour\n").unwrap();

    test.keys(" gd");
    assert!(test.editor.is_diff_review_buffer());
    assert_eq!(test.editor.current_tab_index(), 1);
    assert_eq!(
        current_line(&test),
        "+four",
        "cursor follows the source line through the refresh"
    );
    let text = test.editor.buffer().rope().to_string();
    assert!(
        text.contains("+zero"),
        "review picked up the new change: {text}"
    );

    // Leaving the review returns to the file tab.
    test.keys(" gd");
    assert!(!test.editor.is_diff_review_buffer());
    assert_eq!(test.editor.current_tab_index(), 0);
    assert!(test.editor.buffer().file_path().unwrap().ends_with("a.txt"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn q_closes_the_review_and_drops_its_buffer() {
    let fixture = Fixture::new();
    let mut test = open_editor_on(&fixture, "a.txt");
    let buffers_before = test.editor.buffer_count();
    test.keys(" gd");
    assert_eq!(test.editor.buffer_count(), buffers_before + 1);

    test.keys("q");

    assert!(test.editor.diff_review().is_none());
    assert_eq!(test.editor.tab_count(), 1);
    assert_eq!(test.editor.buffer_count(), buffers_before);
    assert!(test.editor.buffer().file_path().unwrap().ends_with("a.txt"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn gitdiff_command_accepts_an_explicit_base() {
    let fixture = Fixture::new();
    let mut test = open_editor_on(&fixture, "a.txt");

    let result = ovim_core::commands::execute_command(&mut test.editor, "GitDiff HEAD");
    assert!(
        matches!(result, ovim_core::CommandResult::Success(_)),
        "{result:?}"
    );

    assert!(test.editor.is_diff_review_buffer());
    let text = test.editor.buffer().rope().to_string();
    assert!(text.starts_with("feature → HEAD\n"), "{text}");
    assert!(
        text.contains("+new file"),
        "uncommitted b.txt is included: {text}"
    );
    assert!(
        !text.contains("+four"),
        "committed work is excluded: {text}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn on_the_default_branch_the_review_shows_uncommitted_changes() {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    let repo = Repository::init(&root).unwrap();
    repo.set_head("refs/heads/main").unwrap();
    fs::write(root.join("a.txt"), "one\n").unwrap();
    commit_all(&repo, "c1");
    fs::write(root.join("a.txt"), "one\nuncommitted\n").unwrap();

    let mut test = EditorTest::new("");
    test.editor.open_file(root.join("a.txt").as_path()).unwrap();
    test.keys(" gd");

    let text = test.editor.buffer().rope().to_string();
    assert!(text.starts_with("main → main\n"), "{text}");
    assert!(
        text.contains("On main: uncommitted changes against HEAD"),
        "{text}"
    );
    assert!(text.contains("+uncommitted"), "{text}");
}

/// A repo whose feature branch rewrites one line of a Rust file and adds a
/// tab-indented line, so the split layout has to align both sides.
struct RustFixture {
    _dir: tempfile::TempDir,
    root: std::path::PathBuf,
}

impl RustFixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let repo = Repository::init(&root).unwrap();
        repo.set_head("refs/heads/main").unwrap();
        fs::write(
            root.join("a.rs"),
            "fn main() {\n    let x = 1;\n    println!(\"hi\");\n}\n",
        )
        .unwrap();
        commit_all(&repo, "c1");

        let head = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch("feature", &head, false).unwrap();
        repo.set_head("refs/heads/feature").unwrap();
        fs::write(
            root.join("a.rs"),
            "fn main() {\n    let x = 42;\n\tlet y = \"two\";\n    println!(\"hi\");\n}\n",
        )
        .unwrap();
        commit_all(&repo, "edit a");
        Self { _dir: dir, root }
    }

    fn open(&self) -> EditorTest {
        let mut test = EditorTest::new("");
        test.editor
            .open_file(Path::new(
                &self.root.join("a.rs").to_string_lossy().to_string(),
            ))
            .unwrap();
        test
    }
}

fn buffer_text(test: &EditorTest) -> String {
    test.editor.buffer().rope().to_string()
}

/// First buffer line whose text contains `needle`.
fn line_containing(test: &EditorTest, needle: &str) -> usize {
    let buffer = test.editor.buffer();
    (0..buffer.line_count())
        .find(|&index| {
            buffer
                .line_text(index)
                .is_some_and(|text| text.contains(needle))
        })
        .unwrap_or_else(|| panic!("no line containing {needle:?} in review"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn the_patch_is_highlighted_with_each_file_s_own_grammar() {
    let fixture = RustFixture::new();
    let mut test = fixture.open();
    test.keys(" gd");

    // `let` inside an added line is a Rust keyword, not "an added line".
    let added = line_index_of(&test, "+    let x = 42;");
    let highlights = test.editor.buffer().highlights_for_line(added);
    assert!(
        highlights.iter().any(|(range, group)| *group
            == ovim_core::syntax::HighlightGroup::Keyword
            && range.start == 5),
        "expected a keyword span over `let`: {highlights:?}"
    );
    // The marker column still carries the diff colour.
    assert!(highlights
        .iter()
        .any(|(range, group)| *range == (0..1)
            && *group == ovim_core::syntax::HighlightGroup::DiffAdded));

    // Removed lines are highlighted from the old side of the patch.
    let removed = line_index_of(&test, "-    let x = 1;");
    assert!(test
        .editor
        .buffer()
        .highlights_for_line(removed)
        .iter()
        .any(|(_, group)| *group == ovim_core::syntax::HighlightGroup::Keyword));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn changed_rows_carry_a_background_tint_and_context_rows_do_not() {
    let fixture = RustFixture::new();
    let mut test = fixture.open();
    test.keys(" gd");

    let added = line_index_of(&test, "+    let x = 42;");
    let removed = line_index_of(&test, "-    let x = 1;");
    let context = line_index_of(&test, " fn main() {");
    let review = test.editor.diff_review().unwrap();
    assert_eq!(review.line_tints(added).len(), 1);
    assert!(review.line_tints(added)[0].1, "added rows tint green");
    assert!(!review.line_tints(removed)[0].1, "removed rows tint red");
    assert!(review.line_tints(context).is_empty());

    // The band runs to the end of the row so the renderer can carry it
    // through the padding to the right edge.
    assert_eq!(review.line_trailing_tint(added), Some(true));
    assert_eq!(review.line_trailing_tint(removed), Some(false));
    assert_eq!(review.line_trailing_tint(context), None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn only_the_side_that_changed_is_tinted_in_the_split_layout() {
    let fixture = RustFixture::new();
    let mut test = fixture.open();
    test.keys(" gds");

    // A rewritten line pairs a removal on the left with an addition on the
    // right, so the row carries both tints and the band ends on the new side.
    let row = line_containing(&test, "let x = 1;");
    let review = test.editor.diff_review().unwrap();
    let tints = review.line_tints(row);
    assert_eq!(tints.len(), 2, "{tints:?}");
    assert!(!tints[0].1, "the old column is a removal");
    assert!(tints[1].1, "the new column is an addition");
    assert!(
        tints[0].0.end < tints[1].0.start,
        "the separator is untinted"
    );
    assert_eq!(review.line_trailing_tint(row), Some(true));

    // A row that only adds leaves the old column plain.
    let added_only = line_containing(&test, "let y = \"two\";");
    let tints = test.editor.diff_review().unwrap().line_tints(added_only);
    assert_eq!(tints.len(), 1, "{tints:?}");
    assert!(tints[0].1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn s_switches_between_the_unified_and_split_layouts() {
    let fixture = RustFixture::new();
    let mut test = fixture.open();
    test.keys(" gd");
    assert!(buffer_text(&test).contains("diff --git a/a.rs b/a.rs"));
    assert!(!buffer_text(&test).contains('│'));

    test.keys("s");

    let text = buffer_text(&test);
    assert!(
        text.contains('│'),
        "side-by-side rows are separated: {text}"
    );
    assert!(
        text.contains("── a.rs "),
        "the raw file header becomes a banner: {text}"
    );
    assert!(
        !text.contains("diff --git"),
        "the git header is folded into the banner: {text}"
    );
    // Both sides of the rewritten line share a row, with their line numbers.
    let row = line_containing(&test, "let x = 1;");
    let row_text = test.editor.buffer().line_text(row).unwrap().to_string();
    assert!(row_text.contains("let x = 42;"), "{row_text}");
    assert!(row_text.starts_with("  2 -"), "{row_text}");

    test.keys("s");
    assert!(buffer_text(&test).contains("diff --git a/a.rs b/a.rs"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn split_rows_align_after_expanding_tabs() {
    let fixture = RustFixture::new();
    let mut test = fixture.open();
    test.keys(" gds");

    // The added line is indented with a tab; both sides must still line up on
    // the separator column.
    let rows: Vec<String> = (0..test.editor.buffer().line_count())
        .filter_map(|index| {
            test.editor
                .buffer()
                .line_text(index)
                .map(|text| text.to_string())
        })
        .filter(|text| text.contains('│'))
        .collect();
    assert!(rows.len() >= 4, "{rows:?}");
    let columns: Vec<usize> = rows
        .iter()
        .map(|text| text.chars().position(|c| c == '│').unwrap())
        .collect();
    assert!(
        columns.windows(2).all(|pair| pair[0] == pair[1]),
        "every row separates at the same column: {columns:?}"
    );
    assert!(
        rows.iter()
            .any(|text| text.contains("    let y = \"two\";")),
        "the tab is expanded, not passed through: {rows:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn enter_in_the_split_layout_opens_the_column_under_the_cursor() {
    let fixture = RustFixture::new();
    let mut test = fixture.open();
    test.keys(" gds");

    let row = line_containing(&test, "let x = 42;");
    let text = test.editor.buffer().line_text(row).unwrap().to_string();
    // Land on the `4` of `42` on the new side. Columns are graphemes, and the
    // row's separator is multi-byte.
    let col = text[..text.find("42;").unwrap()].chars().count();
    test.set_cursor(row, col);
    test.press_key(KeyCode::Enter);

    assert!(test.editor.buffer().file_path().unwrap().ends_with("a.rs"));
    let cursor = test.editor.buffer().cursor();
    assert_eq!(cursor.line(), 1, "the new side's line 2");
    assert_eq!(
        current_line(&test)
            .chars()
            .nth(cursor.col().0)
            .unwrap_or(' '),
        '4',
        "cursor keeps its column: {:?}",
        current_line(&test)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn enter_on_the_old_column_lands_where_the_removal_happened() {
    let fixture = RustFixture::new();
    let mut test = fixture.open();
    test.keys(" gds");

    let row = line_containing(&test, "let x = 1;");
    let text = test.editor.buffer().line_text(row).unwrap().to_string();
    let col = text[..text.find("let x = 1;").unwrap()].chars().count();
    test.set_cursor(row, col);
    test.press_key(KeyCode::Enter);

    assert!(test.editor.buffer().file_path().unwrap().ends_with("a.rs"));
    assert_eq!(test.editor.buffer().cursor().line(), 1);
    assert_eq!(current_line(&test), "    let x = 42;");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn clicking_the_toolbar_switches_layout_without_moving_the_cursor() {
    let fixture = RustFixture::new();
    let mut test = fixture.open();
    test.keys(" gd");

    let toolbar = line_containing(&test, "[ Split ]");
    let text = test.editor.buffer().line_text(toolbar).unwrap().to_string();
    let split_col = text.find("[ Split ]").unwrap() + 2;
    let unified_col = text.find("[ Unified ]").unwrap() + 2;
    let hint_col = text.find("· click").unwrap();

    assert!(test.editor.diff_review_click(toolbar, split_col));
    assert!(buffer_text(&test).contains('│'));

    assert!(test.editor.diff_review_click(toolbar, unified_col));
    assert!(buffer_text(&test).contains("diff --git"));

    assert!(
        !test.editor.diff_review_click(toolbar, hint_col),
        "clicks outside the buttons fall through to the cursor"
    );
    assert!(
        !test.editor.diff_review_click(toolbar + 1, split_col),
        "only the toolbar row is clickable"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn switching_layout_keeps_the_cursor_on_the_same_change() {
    let fixture = RustFixture::new();
    let mut test = fixture.open();
    test.keys(" gd");

    let added = line_index_of(&test, "+\tlet y = \"two\";");
    test.set_cursor(added, 0);
    test.keys("s");
    assert!(
        current_line(&test).contains("let y = \"two\";"),
        "cursor followed the change into the split view: {:?}",
        current_line(&test)
    );

    test.keys("s");
    assert_eq!(current_line(&test), "+\tlet y = \"two\";");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn hunk_and_file_navigation_work_in_the_split_layout() {
    let fixture = Fixture::new();
    let mut test = open_editor_on(&fixture, "a.txt");
    test.keys(" gds");

    test.keys("]f");
    assert!(
        current_line(&test).contains("── a.txt "),
        "{}",
        current_line(&test)
    );
    test.keys("]f");
    assert!(
        current_line(&test).contains("── b.txt "),
        "{}",
        current_line(&test)
    );
    test.keys("[f");
    assert!(
        current_line(&test).contains("── a.txt "),
        "{}",
        current_line(&test)
    );

    test.keys("]c");
    assert_eq!(current_line(&test), "@@ -1,3 +1,4 @@");
    test.keys("]c");
    assert_eq!(current_line(&test), "@@ -0,0 +1 @@");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn the_layout_choice_survives_closing_and_reopening_the_review() {
    let fixture = RustFixture::new();
    let mut test = fixture.open();
    test.keys(" gds");
    test.keys("q");
    assert!(test.editor.diff_review().is_none());

    test.keys(" gd");
    assert!(
        buffer_text(&test).contains('│'),
        "the next review opens in the layout you last chose"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn gitdifflayout_command_sets_the_layout_explicitly() {
    let fixture = RustFixture::new();
    let mut test = fixture.open();
    test.keys(" gd");

    let result = ovim_core::commands::execute_command(&mut test.editor, "GitDiffLayout split");
    assert!(
        matches!(result, ovim_core::CommandResult::Success(_)),
        "{result:?}"
    );
    assert!(buffer_text(&test).contains('│'));

    let result = ovim_core::commands::execute_command(&mut test.editor, "GitDiffLayout sideways");
    assert!(
        matches!(result, ovim_core::CommandResult::Error(_)),
        "{result:?}"
    );
    assert!(
        buffer_text(&test).contains('│'),
        "an invalid name changes nothing"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn the_split_layout_handles_a_branch_with_no_changes() {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    let repo = Repository::init(&root).unwrap();
    repo.set_head("refs/heads/main").unwrap();
    fs::write(root.join("a.txt"), "one\n").unwrap();
    commit_all(&repo, "c1");
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    repo.branch("feature", &head, false).unwrap();
    repo.set_head("refs/heads/feature").unwrap();

    let mut test = EditorTest::new("");
    test.editor.open_file(root.join("a.txt").as_path()).unwrap();
    test.keys(" gds");

    let text = buffer_text(&test);
    assert!(text.contains("No changes"), "{text}");
    // Navigation on an empty review reports rather than panics.
    test.keys("]c");
    assert_eq!(test.editor.status_message(), "Last hunk");
    test.keys("]f");
    assert_eq!(test.editor.status_message(), "Last file");
    test.press_key(KeyCode::Enter);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn the_split_layout_shows_deletions_and_binaries() {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    let repo = Repository::init(&root).unwrap();
    repo.set_head("refs/heads/main").unwrap();
    fs::write(root.join("gone.txt"), "one\ntwo\n").unwrap();
    fs::write(root.join("keep.txt"), "keep\n").unwrap();
    commit_all(&repo, "c1");

    let head = repo.head().unwrap().peel_to_commit().unwrap();
    repo.branch("feature", &head, false).unwrap();
    repo.set_head("refs/heads/feature").unwrap();
    fs::remove_file(root.join("gone.txt")).unwrap();
    fs::write(root.join("blob.bin"), [0u8, 159, 146, 150, 0]).unwrap();
    commit_all(&repo, "delete and add a binary");

    let mut test = EditorTest::new("");
    test.editor
        .open_file(root.join("keep.txt").as_path())
        .unwrap();
    test.keys(" gds");

    let text = buffer_text(&test);
    assert!(text.contains("── blob.bin "), "{text}");
    assert!(text.contains("── gone.txt "), "{text}");
    assert!(text.contains("D  +0 −2"), "{text}");

    // Enter on a deleted file refuses instead of opening a missing path.
    let row = line_containing(&test, "- one");
    test.set_cursor(row, 8);
    test.press_key(KeyCode::Enter);
    assert!(test.editor.is_diff_review_buffer());
    assert!(test
        .editor
        .buffer()
        .file_path()
        .is_none_or(|path| !path.ends_with("gone.txt")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn a_long_line_wraps_inside_its_split_column() {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    let repo = Repository::init(&root).unwrap();
    repo.set_head("refs/heads/main").unwrap();
    fs::write(root.join("a.txt"), "short\n").unwrap();
    commit_all(&repo, "c1");
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    repo.branch("feature", &head, false).unwrap();
    repo.set_head("refs/heads/feature").unwrap();
    let long = "x".repeat(1200);
    fs::write(root.join("a.txt"), format!("short\n{long}\n")).unwrap();
    commit_all(&repo, "long line");

    let mut test = EditorTest::new("");
    test.editor.open_file(root.join("a.txt").as_path()).unwrap();
    test.keys(" gds");

    let rows: Vec<String> = (0..test.editor.buffer().line_count())
        .filter_map(|index| test.editor.buffer().line_text(index).map(|t| t.to_string()))
        .filter(|text| text.contains("xxx"))
        .collect();
    assert!(rows.len() > 1, "the long line wraps: {}", rows.len());
    // Wrapping is capped, and the clipped row says so.
    assert!(rows.len() <= 12, "wrapping is bounded: {}", rows.len());
    assert!(
        rows.last().unwrap().contains('…'),
        "the clipped row is marked: {:?}",
        rows.last()
    );
    // Every wrapped row still ends at the same separator column.
    let columns: Vec<usize> = rows
        .iter()
        .map(|text| text.chars().position(|c| c == '│').unwrap())
        .collect();
    assert!(
        columns.windows(2).all(|pair| pair[0] == pair[1]),
        "{columns:?}"
    );

    // Enter from a continuation row lands on the same source line.
    let row = line_containing(&test, "xxx");
    test.set_cursor(row + 1, 70);
    test.press_key(KeyCode::Enter);
    assert_eq!(test.editor.buffer().cursor().line(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn pullbase_controls_review_and_unset_restores_auto() {
    use ovim_core::command_result::CommandResult;
    use ovim_core::commands::execute_command;
    let fixture = Fixture::new();
    let mut test = open_editor_on(&fixture, "a.txt");
    for command in ["set pullbase=feature", "GitDiff"] {
        assert!(matches!(
            execute_command(&mut test.editor, command),
            CommandResult::Success(_)
        ));
    }
    assert_eq!(test.editor.diff_review().unwrap().base().name, "feature");
    execute_command(&mut test.editor, "set pullbase=main");
    assert_eq!(test.editor.diff_review().unwrap().base().name, "main");
    execute_command(&mut test.editor, "GitDiff feature");
    execute_command(&mut test.editor, "unset pullbase");
    assert_eq!(test.editor.diff_review().unwrap().base().name, "feature");
    execute_command(&mut test.editor, "GitDiff");
    assert_eq!(test.editor.diff_review().unwrap().base().name, "main");
    for unset in [
        "set pullbase=",
        "set pullbase&",
        "set nopullbase",
        "unset pullbase",
    ] {
        execute_command(&mut test.editor, "set pullbase=feature");
        assert!(matches!(
            execute_command(&mut test.editor, unset),
            CommandResult::Success(_)
        ));
        assert_eq!(test.editor.options.pullbase, None);
    }
    assert!(matches!(
        execute_command(&mut test.editor, "set pullbase=main..feature"),
        CommandResult::Error(_)
    ));
    assert_eq!(test.editor.options.pullbase, None);
    let result = execute_command(&mut test.editor, "set pullbase?");
    assert!(
        matches!(result, CommandResult::Success(ref response) if response.message.as_deref() == Some("  pullbase="))
    );
}

#[test]
fn pullbase_resolves_remote_metadata_and_reports_missing_branches() {
    let fixture = Fixture::new();
    let repo = Repository::open(&fixture.root).unwrap();
    let oid = repo.refname_to_id("refs/heads/main").unwrap();
    repo.reference("refs/remotes/origin/release/stable", oid, true, "test")
        .unwrap();
    let base =
        ovim_core::native_diff::resolve_pullbase(&fixture.root, Some("origin/release/stable"))
            .unwrap();
    assert_eq!(
        base.remote,
        Some(("origin".into(), "release/stable".into()))
    );
    assert_eq!(base.spec, "origin/release/stable...WORKTREE");
    assert!(ovim_core::native_diff::resolve_pullbase(&fixture.root, Some("missing")).is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn pullbase_path_override_refreshes_and_unsets_without_changing_global() {
    use ovim_core::command_result::CommandResult;
    use ovim_core::commands::execute_command;
    let fixture = Fixture::new();
    let mut test = open_editor_on(&fixture, "a.txt");
    execute_command(&mut test.editor, "set pullbase=main");
    execute_command(&mut test.editor, "GitDiff");
    let path = fixture.root.display();
    assert!(matches!(
        execute_command(
            &mut test.editor,
            &format!("set pullbase=feature path={path}")
        ),
        CommandResult::Success(_)
    ));
    assert_eq!(test.editor.diff_review().unwrap().base().name, "feature");
    assert_eq!(test.editor.options.pullbase.as_deref(), Some("main"));
    test.editor.refresh_diff_review();
    assert_eq!(test.editor.diff_review().unwrap().base().name, "feature");
    let query = execute_command(&mut test.editor, &format!("set pullbase? path={path}"));
    assert!(matches!(query, CommandResult::Success(ref response)
        if response.message.as_deref() == Some(format!("  pullbase=feature path={path}").as_str())));
    execute_command(&mut test.editor, "GitDiff main");
    assert_eq!(test.editor.diff_review().unwrap().base().name, "main");
    execute_command(&mut test.editor, "GitDiff");
    assert_eq!(test.editor.diff_review().unwrap().base().name, "feature");
    execute_command(&mut test.editor, &format!("unset pullbase path={path}"));
    assert_eq!(test.editor.diff_review().unwrap().base().name, "main");
    assert!(test.editor.options.pullbase_paths.is_empty());
    assert_eq!(test.editor.options.pullbase.as_deref(), Some("main"));
}

#[test]
fn pullbase_path_matching_uses_repo_root_and_closest_directory() {
    use ovim_core::native_diff::pullbase_for_path;
    use std::collections::BTreeMap;
    let fixture = Fixture::new();
    let nested = fixture.root.join("source directory");
    fs::create_dir(&nested).unwrap();
    let mut overrides = BTreeMap::new();
    overrides.insert(
        fixture.root.parent().unwrap().to_path_buf(),
        "parent".into(),
    );
    overrides.insert(fixture.root.clone(), "project".into());
    overrides.insert(nested.clone(), "source".into());
    assert_eq!(
        pullbase_for_path(&nested, Some("global"), &overrides).unwrap(),
        Some("project")
    );
    overrides.remove(&fixture.root);
    assert_eq!(
        pullbase_for_path(&nested, Some("global"), &overrides).unwrap(),
        Some("parent")
    );
    overrides.remove(fixture.root.parent().unwrap());
    assert_eq!(
        pullbase_for_path(&nested, Some("global"), &overrides).unwrap(),
        Some("global")
    );
    assert_eq!(pullbase_for_path(&nested, None, &overrides).unwrap(), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn pullbase_controls_gutter_signs_but_keeps_head_as_the_unset_default() {
    use ovim_core::commands::execute_command;
    use ovim_core::git::GitStatus;
    let fixture = Fixture::new();
    let mut test = open_editor_on(&fixture, "a.txt");
    assert_eq!(test.editor.buffer().git_status().change_counts(), (0, 0, 0));
    // The review still defaults to main, even though the gutter defaults to HEAD.
    execute_command(&mut test.editor, "GitDiff");
    assert_eq!(test.editor.diff_review().unwrap().base().name, "main");
    test.editor.close_diff_review();
    execute_command(&mut test.editor, "set pullbase=main");
    let changes = test.editor.buffer().git_status().change_counts();
    assert_ne!(changes, (0, 0, 0));
    let path = fixture.root.display();
    execute_command(
        &mut test.editor,
        &format!("set pullbase=feature path={path}"),
    );
    assert_eq!(test.editor.buffer().git_status().change_counts(), (0, 0, 0));
    execute_command(&mut test.editor, &format!("unset pullbase path={path}"));
    assert_eq!(test.editor.buffer().git_status().change_counts(), changes);

    // An editor configured before opening a file gets the same signs immediately.
    let mut other = ovim_core::editor::Editor::new();
    execute_command(&mut other, "set pullbase=main");
    other.open_file(Path::new(&fixture.path("a.txt"))).unwrap();
    assert_eq!(other.buffer().git_status().change_counts(), changes);

    // Background save refresh uses the override, and a later unset invalidates it.
    test.editor.spawn_git_refresh(&fixture.path("a.txt"), false);
    let mut refreshed = false;
    for _ in 0..100 {
        if test.editor.poll_git_refresh() {
            refreshed = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(refreshed, "background gutter refresh must complete");
    assert_eq!(test.editor.buffer().git_status().change_counts(), changes);
    test.editor.spawn_git_refresh(&fixture.path("a.txt"), false);
    execute_command(&mut test.editor, "unset pullbase");
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    test.editor.poll_git_refresh();
    assert_eq!(test.editor.buffer().git_status().change_counts(), (0, 0, 0));

    fs::write(fixture.root.join("a.txt"), "uncommitted\n").unwrap();
    assert_ne!(
        GitStatus::from_file(fixture.root.join("a.txt"))
            .unwrap()
            .change_counts(),
        (0, 0, 0)
    );
}

#[test]
fn pullbase_gutter_uses_merge_base_not_unrelated_changes_on_target() {
    use ovim_core::git::GitStatus;
    let fixture = Fixture::new();
    let repo = Repository::open(&fixture.root).unwrap();
    repo.set_head("refs/heads/main").unwrap();
    fs::write(fixture.root.join("a.txt"), "unrelated target changes\n").unwrap();
    commit_all(&repo, "advance target independently");
    repo.set_head("refs/heads/feature").unwrap();
    fs::write(fixture.root.join("a.txt"), "one\n2\nthree\nfour\n").unwrap();
    let mut checkout = git2::build::CheckoutBuilder::new();
    checkout.force();
    repo.checkout_head(Some(&mut checkout)).unwrap();
    let signs =
        GitStatus::from_file_with_pullbase(fixture.root.join("a.txt"), Some("main")).unwrap();
    assert_eq!(
        signs.get_line_status(0),
        None,
        "unchanged first line must not be marked"
    );
    assert!(signs.get_line_status(3).is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn blame_mouse_hover_renders_details_without_moving_cursor_or_taking_keyboard() {
    use ovim::editor::handle_mouse_event;
    use ovim::ui::Renderer;
    use ovim_core::{MouseEvent, MouseEventKind, Rect};
    use ratatui::{backend::TestBackend, Terminal};
    let fixture = Fixture::new();
    let mut test = open_editor_on(&fixture, "a.txt");
    test.editor.options.blame = true;
    test.editor.options.wrap = false;
    test.editor.buffer_mut().load_git_blame();
    test.editor.render_cache.last_buffer_area = Some(Rect {
        x: 0,
        y: 0,
        width: 100,
        height: 25,
    });
    test.editor.render_cache.last_blame_width = 20;
    let mouse = |row, column| MouseEvent {
        kind: MouseEventKind::Moved,
        row,
        column,
    };
    handle_mouse_event(&mut test.editor, mouse(1, 1)).unwrap();
    assert_eq!(test.editor.mode(), ovim_core::mode::Mode::Normal);
    assert_eq!(test.editor.buffer().cursor().line(), 0);
    assert_eq!(test.editor.hover_position(), Some((1, 0)));
    assert!(test.editor.hover_info().unwrap().contains("edit a"));
    test.editor.mark_clean();
    handle_mouse_event(&mut test.editor, mouse(1, 2)).unwrap();
    assert!(
        !test.editor.is_dirty(),
        "moving within the same annotation must not redraw"
    );
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal
        .draw(|frame| Renderer::render_to_frame(frame, &mut test.editor, &mut Default::default()))
        .unwrap();
    let rendered: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(rendered.contains("Author:"));
    assert!(rendered.contains("edit a"));
    handle_mouse_event(&mut test.editor, mouse(1, 99)).unwrap();
    assert!(test.editor.hover_info().is_none());
    handle_mouse_event(&mut test.editor, mouse(1, 1)).unwrap();
    test.keys("j");
    assert_eq!(
        test.editor.buffer().cursor().line(),
        1,
        "j must move the editing cursor"
    );
    assert!(test.editor.hover_info().is_none());
    handle_mouse_event(&mut test.editor, mouse(20, 1)).unwrap();
    assert!(
        test.editor.hover_info().is_none(),
        "blank gutter rows have no annotation"
    );
    test.keys("i");
    handle_mouse_event(&mut test.editor, mouse(1, 1)).unwrap();
    assert_eq!(test.editor.mode(), ovim_core::mode::Mode::Insert);
    assert!(test.editor.hover_info().is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn clicking_blame_opens_that_commit_and_preserves_source_cursor() {
    use ovim::editor::handle_mouse_event;
    use ovim_core::{MouseButton, MouseEvent, MouseEventKind, Rect};
    let fixture = Fixture::new();
    let repo = Repository::open(&fixture.root).unwrap();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    for (row, oid) in [(0, head.parent_id(0).unwrap()), (1, head.id())] {
        let mut test = open_editor_on(&fixture, "a.txt");
        test.editor.options.blame = true;
        test.editor.options.wrap = false;
        test.editor.buffer_mut().load_git_blame();
        let source_id = test.editor.buffer().id();
        let source_cursor = *test.editor.buffer().cursor();
        test.editor.render_cache.last_buffer_area = Some(Rect {
            x: 2,
            y: 3,
            width: 100,
            height: 25,
        });
        test.editor.render_cache.last_blame_width = 20;
        handle_mouse_event(
            &mut test.editor,
            MouseEvent {
                kind: MouseEventKind::Moved,
                row: row + 3,
                column: 3,
            },
        )
        .unwrap();
        assert!(test.editor.blame_mouse_hover_active());
        handle_mouse_event(
            &mut test.editor,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                row: row + 3,
                column: 3,
            },
        )
        .unwrap();
        assert_eq!(test.editor.tab_count(), 2);
        assert_eq!(test.editor.mode(), ovim_core::mode::Mode::Normal);
        assert!(test.editor.hover_info().is_none());
        assert!(test.editor.buffer().is_read_only());
        assert!(test.editor.buffer().file_path().is_none());
        let diff = test.editor.buffer().rope().to_string();
        assert!(diff.starts_with(&format!("commit {oid}\n")), "{diff}");
        assert!(diff.contains("diff --git a/a.txt b/a.txt"));
        assert!(
            !diff.contains("b.txt"),
            "uncommitted files do not belong to a commit patch"
        );
        let added = diff
            .lines()
            .position(|line| line == if row == 0 { "+two" } else { "+2" })
            .unwrap();
        assert!(!test.editor.buffer().highlights_for_line(added).is_empty());
        test.keys("gT");
        assert_eq!(test.editor.buffer().id(), source_id);
        assert_eq!(*test.editor.buffer().cursor(), source_cursor);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn blame_bands_color_every_row_in_a_commit_group_and_ignore_empty_wrap_rows() {
    use ovim::editor::handle_mouse_event;
    use ovim::ui::Renderer;
    use ovim_core::{MouseButton, MouseEvent, MouseEventKind};
    use ratatui::{backend::TestBackend, Terminal};
    let fixture = Fixture::new();
    let mut test = open_editor_on(&fixture, "a.txt");
    test.editor.options.blame = true;
    test.editor.options.wrap = true;
    test.editor.buffer_mut().load_git_blame();
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal
        .draw(|frame| Renderer::render_to_frame(frame, &mut test.editor, &mut Default::default()))
        .unwrap();
    let area = test.editor.render_cache.last_buffer_area.unwrap();
    let screen = terminal.backend().buffer();
    // Lines 1 and 3 are from the same original commit; both have a colored band.
    let first = &screen[(area.x, area.y)];
    let third = &screen[(area.x, area.y + 2)];
    assert_eq!(first.bg, third.bg);
    assert_eq!(first.fg, third.fg);
    assert_ne!(first.bg, screen[(area.x + 90, area.y)].bg);
    let tabs = test.editor.tab_count();
    handle_mouse_event(
        &mut test.editor,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            row: area.y + 20,
            column: area.x + 1,
        },
    )
    .unwrap();
    assert_eq!(test.editor.tab_count(), tabs);
    assert!(test.editor.hover_info().is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn wrapped_blame_continuations_keep_the_commit_band_and_click_target() {
    use ovim::editor::handle_mouse_event;
    use ovim::ui::Renderer;
    use ovim_core::{MouseButton, MouseEvent, MouseEventKind};
    use ratatui::{backend::TestBackend, Terminal};
    let fixture = Fixture::new();
    fs::write(
        fixture.root.join("a.txt"),
        format!("{}\nsecond\n", "long text ".repeat(30)),
    )
    .unwrap();
    let repo = Repository::open(&fixture.root).unwrap();
    let oid = commit_all(&repo, "long line");
    let mut test = open_editor_on(&fixture, "a.txt");
    test.editor.options.blame = true;
    test.editor.options.wrap = true;
    test.editor.buffer_mut().load_git_blame();
    let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
    terminal
        .draw(|frame| Renderer::render_to_frame(frame, &mut test.editor, &mut Default::default()))
        .unwrap();
    let area = test.editor.render_cache.last_buffer_area.unwrap();
    let screen = terminal.backend().buffer();
    assert_eq!(screen[(area.x, area.y)].bg, screen[(area.x, area.y + 1)].bg);
    handle_mouse_event(
        &mut test.editor,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            row: area.y + 1,
            column: area.x + 1,
        },
    )
    .unwrap();
    assert!(test
        .editor
        .buffer()
        .rope()
        .to_string()
        .starts_with(&format!("commit {oid}\n")));
}
