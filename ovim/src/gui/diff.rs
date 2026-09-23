//! Structured diff data for the native GUI. The review patch remains owned by
//! core; this projection only replaces the terminal's width-dependent paint.

use serde::{Serialize, Serializer};
use std::path::{Component, Path};
use std::sync::Arc;

use crate::buffer::Buffer;
use crate::editor::{DiffLayout, DiffReviewState, Editor, DIFF_REVIEW_TITLE_PREFIX};
use ovim_core::native_diff::{CustomReview, PatchLineKind, ReviewPatch};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuiDiffReview {
    pub title: String,
    pub layout: &'static str,
    pub managed: bool,
    pub custom: bool,
    pub files: Vec<GuiDiffFile>,
}

/// A cached projection shared by successive snapshots. Cursor movement does
/// not need to clone a multi-megabyte patch just to send the next snapshot.
#[derive(Debug, Clone)]
pub struct GuiDiffDocument(pub Arc<GuiDiffReview>);

impl PartialEq for GuiDiffDocument {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0) || self.0 == other.0
    }
}

impl Serialize for GuiDiffDocument {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.as_ref().serialize(serializer)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuiDiffFile {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    pub status: String,
    pub additions: usize,
    pub deletions: usize,
    pub binary: bool,
    pub metadata: Vec<String>,
    pub hunks: Vec<GuiDiffHunk>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuiDiffHunk {
    pub header: String,
    pub old_start: usize,
    pub old_count: usize,
    pub new_start: usize,
    pub new_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_line: Option<usize>,
    pub lines: Vec<GuiDiffLine>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuiDiffLine {
    pub kind: &'static str,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_line: Option<usize>,
    pub highlights: Vec<GuiDiffHighlight>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuiDiffHighlight {
    pub start: usize,
    pub end: usize,
    pub token: String,
}

pub fn project_diff(editor: &Editor, buffer: &Buffer) -> Option<GuiDiffReview> {
    let title = buffer.display_name()?.to_string();
    if let Some(review) = editor
        .diff_review()
        .filter(|state| state.buffer_id == buffer.id())
    {
        if let Some(custom) = review.custom() {
            return Some(project_custom_review(&title, review, custom));
        }
        return Some(project_review_patch(&title, review));
    }
    let path = title.strip_prefix(DIFF_REVIEW_TITLE_PREFIX)?;
    Some(project_standalone(&title, path, &buffer.rope().to_string()))
}

fn project_review_patch(title: &str, review: &DiffReviewState) -> GuiDiffReview {
    let patch: &ReviewPatch = review.patch();
    let review_lines = review.patch_review_lines();
    let mut files: Vec<GuiDiffFile> = patch
        .files
        .iter()
        .map(|file| GuiDiffFile {
            id: file.path.clone(),
            label: None,
            path: file.path.clone(),
            old_path: file.old_path.clone(),
            status: file.status.clone(),
            additions: file.additions,
            deletions: file.deletions,
            binary: file.binary,
            metadata: Vec::new(),
            hunks: Vec::new(),
        })
        .collect();

    for (index, (text, info)) in patch.text.lines().zip(&patch.lines).enumerate() {
        let Some(file) = info.file.and_then(|index| files.get_mut(index)) else {
            continue;
        };
        match info.kind {
            PatchLineKind::HunkHeader => {
                let (old_start, old_count, new_start, new_count) =
                    parse_hunk_header(text).unwrap_or((0, 0, 0, 0));
                file.hunks.push(GuiDiffHunk {
                    header: text.to_string(),
                    old_start,
                    old_count,
                    new_start,
                    new_count,
                    review_line: review_lines.get(index).copied().flatten(),
                    lines: Vec::new(),
                });
            }
            PatchLineKind::Context | PatchLineKind::Added | PatchLineKind::Removed => {
                let Some(hunk) = file.hunks.last_mut() else {
                    continue;
                };
                hunk.lines.push(GuiDiffLine {
                    kind: kind_name(info.kind),
                    text: text.get(1..).unwrap_or("").to_string(),
                    old_line: info.old_line,
                    new_line: info.new_line,
                    review_line: review_lines.get(index).copied().flatten(),
                    highlights: project_highlights(
                        text.get(1..).unwrap_or(""),
                        review.patch_line_highlights(index),
                    ),
                });
            }
            PatchLineKind::FileHeader | PatchLineKind::Meta => {
                if is_visible_metadata(text) {
                    file.metadata.push(text.to_string());
                }
            }
        }
    }

    GuiDiffReview {
        title: title.to_string(),
        layout: layout_name(review.layout),
        managed: true,
        custom: false,
        files,
    }
}

fn project_custom_review(
    title: &str,
    review: &DiffReviewState,
    custom: &CustomReview,
) -> GuiDiffReview {
    let patch = review.patch();
    let review_lines = review.patch_review_lines();
    let files = custom
        .sections
        .iter()
        .map(|section| {
            let path = section
                .new_path
                .as_deref()
                .or(section.old_path.as_deref())
                .unwrap_or("(metadata)")
                .to_string();
            let canonical = patch.files.iter().find(|file| {
                section.new_path.as_deref() == Some(file.path.as_str())
                    || section.old_path.as_deref() == Some(file.path.as_str())
                    || (section.old_path.is_some()
                        && section.old_path.as_deref() == file.old_path.as_deref())
            });
            let additions = section
                .lines
                .iter()
                .filter(|line| line.kind == PatchLineKind::Added)
                .count();
            let deletions = section
                .lines
                .iter()
                .filter(|line| line.kind == PatchLineKind::Removed)
                .count();
            let header = section
                .lines
                .iter()
                .find(|line| line.kind == PatchLineKind::HunkHeader)
                .map(|line| line.text.clone())
                .or_else(|| section.label.clone())
                .unwrap_or_else(|| {
                    format!(
                        "{} → {}",
                        section.old_path.as_deref().unwrap_or("∅"),
                        section.new_path.as_deref().unwrap_or("∅")
                    )
                });
            let old_start = section
                .lines
                .iter()
                .filter_map(|line| line.old_line)
                .min()
                .unwrap_or(0);
            let new_start = section
                .lines
                .iter()
                .filter_map(|line| line.new_line)
                .min()
                .unwrap_or(0);
            let lines: Vec<GuiDiffLine> = section
                .lines
                .iter()
                .filter(|line| {
                    matches!(
                        line.kind,
                        PatchLineKind::Context | PatchLineKind::Added | PatchLineKind::Removed
                    )
                })
                .map(|line| GuiDiffLine {
                    kind: kind_name(line.kind),
                    text: line.text.clone(),
                    old_line: (line.kind != PatchLineKind::Added)
                        .then_some(line.old_line)
                        .flatten(),
                    new_line: (line.kind != PatchLineKind::Removed)
                        .then_some(line.new_line)
                        .flatten(),
                    review_line: review_lines.get(line.source_patch_line).copied().flatten(),
                    highlights: project_highlights(
                        &line.text,
                        review.patch_line_highlights(line.source_patch_line),
                    ),
                })
                .collect();
            let hunks = if lines.is_empty() {
                Vec::new()
            } else {
                vec![GuiDiffHunk {
                    header,
                    old_start,
                    old_count: lines.iter().filter(|line| line.old_line.is_some()).count(),
                    new_start,
                    new_count: lines.iter().filter(|line| line.new_line.is_some()).count(),
                    review_line: lines.iter().find_map(|line| line.review_line),
                    lines,
                }]
            };
            GuiDiffFile {
                id: section.id.clone(),
                label: section.label.clone(),
                path,
                old_path: section.old_path.clone(),
                status: if section.is_reassigned {
                    "reassigned".to_string()
                } else {
                    canonical
                        .map(|file| file.status.clone())
                        .unwrap_or_else(|| "modified".to_string())
                },
                additions,
                deletions,
                binary: canonical.is_some_and(|file| file.binary),
                metadata: section
                    .lines
                    .iter()
                    .filter(|line| is_visible_metadata(&line.text))
                    .map(|line| line.text.clone())
                    .collect(),
                hunks,
            }
        })
        .collect();
    GuiDiffReview {
        title: title.to_string(),
        layout: layout_name(review.layout),
        managed: true,
        custom: true,
        files,
    }
}

fn project_standalone(title: &str, path: &str, content: &str) -> GuiDiffReview {
    let mut file = GuiDiffFile {
        id: path.to_string(),
        label: None,
        path: path.to_string(),
        old_path: None,
        status: "modified".to_string(),
        additions: 0,
        deletions: 0,
        binary: false,
        metadata: Vec::new(),
        hunks: Vec::new(),
    };
    let mut old_line = 0;
    let mut new_line = 0;

    for (index, text) in content.lines().enumerate() {
        if let Some((old_start, old_count, new_start, new_count)) = parse_hunk_header(text) {
            old_line = old_start;
            new_line = new_start;
            file.hunks.push(GuiDiffHunk {
                header: text.to_string(),
                old_start,
                old_count,
                new_start,
                new_count,
                review_line: Some(index),
                lines: Vec::new(),
            });
            continue;
        }
        if text.starts_with("Binary files ") || text.starts_with("GIT binary patch") {
            file.binary = true;
        }
        if is_visible_metadata(text) {
            file.metadata.push(text.to_string());
        }
        if let Some(old_path) = text.strip_prefix("rename from ") {
            file.old_path = Some(old_path.to_string());
            file.status = "renamed".to_string();
        }
        if text.starts_with("deleted file mode ") {
            file.status = "deleted".to_string();
        }
        if text.starts_with("new file mode ") {
            file.status = "added".to_string();
        }
        let Some(hunk) = file.hunks.last_mut() else {
            continue;
        };
        let Some((kind, body)) = text
            .strip_prefix('+')
            .map(|body| ("added", body))
            .or_else(|| text.strip_prefix('-').map(|body| ("removed", body)))
            .or_else(|| text.strip_prefix(' ').map(|body| ("context", body)))
        else {
            continue;
        };
        let old = (kind != "added").then_some(old_line);
        let new = (kind != "removed").then_some(new_line);
        if old.is_some() {
            old_line += 1;
        }
        if new.is_some() {
            new_line += 1;
        }
        file.additions += usize::from(kind == "added");
        file.deletions += usize::from(kind == "removed");
        hunk.lines.push(GuiDiffLine {
            kind,
            text: body.to_string(),
            old_line: old,
            new_line: new,
            review_line: Some(index),
            highlights: Vec::new(),
        });
    }

    GuiDiffReview {
        title: title.to_string(),
        layout: "split",
        managed: false,
        custom: false,
        files: vec![file],
    }
}

fn is_visible_metadata(text: &str) -> bool {
    [
        "old mode ",
        "new mode ",
        "new file mode ",
        "deleted file mode ",
        "rename from ",
        "rename to ",
        "copy from ",
        "copy to ",
        "similarity index ",
        "Binary files ",
        "GIT binary patch",
        "\\ No newline at end of file",
    ]
    .iter()
    .any(|prefix| text.starts_with(prefix))
}

fn project_highlights(
    text: &str,
    spans: &[(std::ops::Range<usize>, crate::syntax::HighlightGroup)],
) -> Vec<GuiDiffHighlight> {
    spans
        .iter()
        .filter_map(|(range, group)| {
            let start = range.start.min(text.len());
            let end = range.end.min(text.len());
            if start >= end || !text.is_char_boundary(start) || !text.is_char_boundary(end) {
                return None;
            }
            Some(GuiDiffHighlight {
                start: text[..start].encode_utf16().count(),
                end: text[..end].encode_utf16().count(),
                token: super::syntax_name(*group).to_string(),
            })
        })
        .collect()
}

fn kind_name(kind: PatchLineKind) -> &'static str {
    match kind {
        PatchLineKind::Context => "context",
        PatchLineKind::Added => "added",
        PatchLineKind::Removed => "removed",
        _ => unreachable!("only source lines have a line kind"),
    }
}

fn layout_name(layout: DiffLayout) -> &'static str {
    match layout {
        DiffLayout::Split => "split",
        DiffLayout::Unified => "unified",
    }
}

fn parse_hunk_header(header: &str) -> Option<(usize, usize, usize, usize)> {
    let body = header.strip_prefix("@@ -")?;
    let (old, rest) = body.split_once(" +")?;
    let (new, _) = rest.split_once(" @@")?;
    let parse_range = |value: &str| {
        let (start, count) = value.split_once(',').unwrap_or((value, "1"));
        Some((start.parse().ok()?, count.parse().ok()?))
    };
    let (old_start, old_count) = parse_range(old)?;
    let (new_start, new_count) = parse_range(new)?;
    Some((old_start, old_count, new_start, new_count))
}

fn focus_diff_pane(editor: &mut Editor, pane: usize, buffer_id: u64) -> Result<(), String> {
    if !editor.focus_window(pane) || editor.buffer().id() != buffer_id {
        return Err("The diff pane has changed; try the action again".to_string());
    }
    if !editor.is_diff_review_buffer()
        && !editor
            .buffer()
            .display_name()
            .is_some_and(|name| name.starts_with(DIFF_REVIEW_TITLE_PREFIX))
    {
        return Err("This pane no longer shows a diff".to_string());
    }
    Ok(())
}

pub fn perform_diff_action(
    editor: &mut Editor,
    pane: usize,
    buffer_id: u64,
    action: &str,
) -> Result<(), String> {
    focus_diff_pane(editor, pane, buffer_id)?;
    let managed = editor.is_diff_review_buffer();
    match action {
        "refresh" if managed => editor.refresh_diff_review(),
        "split" if managed => editor.set_diff_review_layout(DiffLayout::Split),
        "unified" if managed => editor.set_diff_review_layout(DiffLayout::Unified),
        "close" if managed => editor.close_diff_review(),
        "close" => editor.close_current_tab(),
        _ => return Err(format!("Unsupported diff action: {action}")),
    }
    Ok(())
}

pub fn open_diff_source(
    editor: &mut Editor,
    pane: usize,
    buffer_id: u64,
    path: &str,
    line: usize,
    side: &str,
) -> Result<(), String> {
    focus_diff_pane(editor, pane, buffer_id)?;
    if editor.is_diff_review_buffer() {
        return editor
            .diff_review_open_source(path, line, side)
            .map_err(|error| format!("{error:#}"));
    }

    let expected_path = editor
        .buffer()
        .display_name()
        .and_then(|title| title.strip_prefix(DIFF_REVIEW_TITLE_PREFIX))
        .ok_or_else(|| "This pane no longer shows a file diff".to_string())?;
    if Path::new(path)
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("The selected source is outside this diff".to_string());
    }
    let workspace = super::projected_workspace_path(editor)
        .ok_or_else(|| "No Git workspace is open".to_string())?;
    let root =
        ovim_core::native_diff::worktree_root(&workspace).map_err(|error| format!("{error:#}"))?;
    let review = project_diff(editor, editor.buffer())
        .ok_or_else(|| "This pane no longer shows a diff".to_string())?;
    let file = &review.files[0];
    if file.status == "deleted" {
        return Err(format!("{} was deleted in this diff", file.path));
    }
    if path != expected_path && !(side == "old" && file.old_path.as_deref() == Some(path)) {
        return Err("The selected source is outside this diff".to_string());
    }
    let new_line = match side {
        "new" => line,
        "old" => map_old_line(&file.hunks, line)
            .ok_or_else(|| format!("Old line is not in this diff: {path}:{line}"))?,
        _ => return Err(format!("Unknown diff side: {side}")),
    };
    let target = root.join(expected_path);
    editor
        .open_file(&target)
        .map_err(|error| error.to_string())?;
    editor
        .buffer_mut()
        .cursor_mut()
        .set_position(new_line.saturating_sub(1), crate::unicode::GraphemeCol(0));
    editor.buffer_mut().validate_cursor_position();
    editor.center_cursor_in_viewport();
    editor.mark_dirty();
    Ok(())
}

fn map_old_line(hunks: &[GuiDiffHunk], old_line: usize) -> Option<usize> {
    for hunk in hunks {
        let mut current_new = hunk.new_start;
        for line in &hunk.lines {
            if line.old_line == Some(old_line) {
                return Some(line.new_line.unwrap_or(current_new).max(1));
            }
            if let Some(next) = line.new_line {
                current_new = next + 1;
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::{IndexAddOption, Repository, Signature};
    use std::fs;

    fn commit(repo: &Repository) {
        let mut index = repo.index().unwrap();
        index.add_all(["*"], IndexAddOption::DEFAULT, None).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let signature = Signature::now("Ovim", "ovim@example.com").unwrap();
        let parent = repo.head().ok().and_then(|head| head.peel_to_commit().ok());
        let parents: Vec<&git2::Commit> = parent.iter().collect();
        repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            "fixture",
            &tree,
            &parents,
        )
        .unwrap();
    }

    #[test]
    fn standalone_patch_projects_compact_hunks_and_source_lines() {
        let review = project_standalone(
            "Diff · src/example.rs",
            "src/example.rs",
            "comparison: HEAD...WORKTREE\nfile: src/example.rs\n\n@@ -8,2 +8,3 @@ fn main()\n old\n-gone\n+new\n+next\n",
        );
        let hunk = &review.files[0].hunks[0];
        assert_eq!((hunk.old_start, hunk.old_count), (8, 2));
        assert_eq!((hunk.new_start, hunk.new_count), (8, 3));
        assert_eq!(hunk.lines[1].old_line, Some(9));
        assert_eq!(hunk.lines[1].new_line, None);
        assert_eq!(hunk.lines[2].new_line, Some(9));
        assert_eq!(hunk.lines[2].review_line, Some(6));
        assert_eq!(map_old_line(&review.files[0].hunks, 9), Some(9));
        let json = serde_json::to_value(&hunk.lines[1]).unwrap();
        assert!(json.get("newLine").is_none());
    }

    #[test]
    fn hunk_header_defaults_omitted_counts_to_one() {
        assert_eq!(parse_hunk_header("@@ -2 +4 @@"), Some((2, 1, 4, 1)));
        assert_eq!(parse_hunk_header("not a hunk"), None);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn branch_projection_keeps_syntax_and_old_line_source_mapping() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let repo = Repository::init(root).unwrap();
        repo.set_head("refs/heads/main").unwrap();
        fs::write(
            root.join("sample.rs"),
            "fn example() {\n    let name = 1;\n}\n",
        )
        .unwrap();
        fs::write(
            root.join("old.rs"),
            "fn renamed() {\n    let first = 1;\n    let second = 2;\n}\n",
        )
        .unwrap();
        fs::write(root.join("gone.rs"), "fn gone() {}\n").unwrap();
        commit(&repo);
        let main = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch("feature", &main, false).unwrap();
        repo.set_head("refs/heads/feature").unwrap();
        fs::write(
            root.join("sample.rs"),
            "fn example() {\n    let name = 2;\n}\n",
        )
        .unwrap();
        fs::rename(root.join("old.rs"), root.join("new.rs")).unwrap();
        fs::write(
            root.join("new.rs"),
            "fn renamed() {\n    let first = 3;\n    let second = 2;\n}\n",
        )
        .unwrap();
        fs::remove_file(root.join("gone.rs")).unwrap();
        {
            let mut index = repo.index().unwrap();
            index.remove_path(Path::new("old.rs")).unwrap();
            index.remove_path(Path::new("gone.rs")).unwrap();
            index.write().unwrap();
        }
        commit(&repo);

        let mut editor = Editor::new();
        editor.open_file(root.join("sample.rs")).unwrap();
        editor.open_diff_review(Some("main")).unwrap();
        let review = project_diff(&editor, editor.buffer()).unwrap();
        assert!(review.managed);
        let sample = review
            .files
            .iter()
            .find(|file| file.path == "sample.rs")
            .unwrap();
        let changed = &sample.hunks[0].lines;
        assert!(changed
            .iter()
            .any(|line| line.kind == "removed" && line.old_line == Some(2)));
        assert!(changed
            .iter()
            .any(|line| line.kind == "added" && line.new_line == Some(2)));
        assert!(changed.iter().any(|line| !line.highlights.is_empty()));
        assert!(changed.iter().all(|line| line.review_line.is_some()));

        editor
            .diff_review_open_source("sample.rs", 2, "old")
            .unwrap();
        assert_eq!(editor.buffer().cursor().line(), 1);
        assert_eq!(editor.buffer().display_name(), None);

        editor.toggle_diff_review();
        let review = project_diff(&editor, editor.buffer()).unwrap();
        let renamed = review
            .files
            .iter()
            .find(|file| file.path == "new.rs")
            .unwrap();
        assert_eq!(renamed.old_path.as_deref(), Some("old.rs"));
        assert_eq!(renamed.status, "renamed");
        editor.diff_review_open_source("old.rs", 2, "old").unwrap();
        assert!(editor.buffer().file_path().unwrap().ends_with("new.rs"));
        assert_eq!(editor.buffer().cursor().line(), 1);

        editor.toggle_diff_review();
        let error = editor
            .diff_review_open_source("gone.rs", 1, "old")
            .unwrap_err();
        assert!(error.to_string().contains("deleted"));
    }

    #[test]
    fn standalone_rename_and_deletion_metadata_are_preserved() {
        let rename = project_standalone(
            "Diff · new.rs",
            "new.rs",
            "rename from old.rs\nrename to new.rs\n@@ -3 +3 @@\n-old\n+new\n",
        );
        assert_eq!(rename.files[0].old_path.as_deref(), Some("old.rs"));
        assert_eq!(rename.files[0].status, "renamed");
        assert_eq!(map_old_line(&rename.files[0].hunks, 3), Some(3));

        let deleted = project_standalone(
            "Diff · gone.rs",
            "gone.rs",
            "deleted file mode 100644\n@@ -1 +0,0 @@\n-gone\n",
        );
        assert_eq!(deleted.files[0].status, "deleted");
    }

    #[test]
    fn stale_buffer_identity_cannot_close_a_new_diff() {
        let mut editor = Editor::new();
        editor.open_diff_buffer_in_new_tab("Diff · sample.rs", "@@ -1 +1 @@\n-old\n+new\n");
        let buffer_id = editor.buffer().id();
        let tab_count = editor.tab_count();
        let error = perform_diff_action(&mut editor, 0, buffer_id + 1, "close").unwrap_err();
        assert!(error.contains("pane has changed"));
        assert_eq!(editor.tab_count(), tab_count);
    }
}
