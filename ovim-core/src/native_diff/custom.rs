//! Frozen review snapshots and explicit reassignment of changed lines.
//!
//! A pairing changes only presentation. Every added and removed line still
//! points at its unique line in the canonical Git patch.

use super::{review_patch, DiffFile, PatchLineKind, ReviewBase, ReviewPatch};
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewSnapshot {
    pub id: String,
    pub patch: ReviewPatch,
    pub blocks: Vec<ChangeBlock>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeBlock {
    pub id: String,
    pub kind: PatchLineKind,
    /// Index in the canonical patch's file summary.
    pub file: usize,
    /// 1-based source line on the side described by `kind`.
    pub start_line: usize,
    pub line_count: usize,
    /// Index of the first line in the canonical patch.
    pub patch_line_start: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeRef {
    pub block_id: String,
    #[serde(default)]
    pub offset: Option<usize>,
    #[serde(default)]
    pub count: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffPairing {
    pub label: Option<String>,
    pub old: ChangeRef,
    pub new: ChangeRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewSectionLine {
    pub kind: PatchLineKind,
    /// Content without the unified patch marker on code lines.
    pub text: String,
    pub old_line: Option<usize>,
    pub new_line: Option<usize>,
    /// Canonical patch line identity, even for a moved presentation line.
    pub source_patch_line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewSection {
    pub id: String,
    pub label: Option<String>,
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub is_reassigned: bool,
    pub lines: Vec<ReviewSectionLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomReview {
    pub snapshot: ReviewSnapshot,
    pub pairings: Vec<DiffPairing>,
    pub sections: Vec<ReviewSection>,
}

pub fn review_snapshot(path: &Path, base: &ReviewBase) -> Result<ReviewSnapshot> {
    ReviewSnapshot::from_patch(review_patch(path, base)?)
}

impl ReviewSnapshot {
    pub fn from_patch(patch: ReviewPatch) -> Result<Self> {
        ensure!(
            !patch.truncated && patch.text.len() <= super::MAX_PATCH_BYTES,
            "Diff exceeds the 4 MiB review limit; narrow the comparison before creating a custom diff"
        );
        let raw: Vec<&str> = patch.text.lines().collect();
        ensure!(
            raw.len() == patch.lines.len(),
            "Git patch text and source map have different line counts"
        );

        let mut blocks: Vec<ChangeBlock> = Vec::new();
        let mut additions = vec![0usize; patch.files.len()];
        let mut deletions = vec![0usize; patch.files.len()];
        for (index, line) in patch.lines.iter().enumerate() {
            let (side, source_line) = match line.kind {
                PatchLineKind::Added => (&mut additions, line.new_line),
                PatchLineKind::Removed => (&mut deletions, line.old_line),
                _ => continue,
            };
            let file = line.file.context("Changed Git line has no file identity")?;
            let source_line = source_line.context("Changed Git line has no source line")?;
            let count = side
                .get_mut(file)
                .context("Changed Git line references a missing file")?;
            *count += 1;
            if let Some(last) = blocks.last_mut() {
                if last.kind == line.kind
                    && last.file == file
                    && last.patch_line_start + last.line_count == index
                {
                    last.line_count += 1;
                    continue;
                }
            }
            blocks.push(ChangeBlock {
                id: format!(
                    "{}_{}",
                    if line.kind == PatchLineKind::Added {
                        "added"
                    } else {
                        "removed"
                    },
                    index
                ),
                kind: line.kind,
                file,
                start_line: source_line,
                line_count: 1,
                patch_line_start: index,
            });
        }
        for (index, file) in patch.files.iter().enumerate() {
            ensure!(
                additions[index] == file.additions && deletions[index] == file.deletions,
                "Git patch for {} is incomplete: expected +{}/-{}, found +{}/-{}",
                file.path,
                file.additions,
                file.deletions,
                additions[index],
                deletions[index]
            );
        }
        let digest = Sha256::digest(serde_json::to_vec(&patch)?);
        let id = format!(
            "diff_{}",
            digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        Ok(Self { id, patch, blocks })
    }

    /// Reassign disjoint line ranges; unassigned lines stay in canonical order.
    pub fn reassign(&self, pairings: &[DiffPairing]) -> Result<CustomReview> {
        let mut assigned = vec![false; self.patch.lines.len()];
        let mut sections = Vec::with_capacity(pairings.len() + self.patch.files.len());
        let raw: Vec<&str> = self.patch.text.lines().collect();

        for (index, pairing) in pairings.iter().enumerate() {
            let old = self.select(&pairing.old, PatchLineKind::Removed, &mut assigned)?;
            let new = self.select(&pairing.new, PatchLineKind::Added, &mut assigned)?;
            let old_file = self.block(&pairing.old.block_id)?.file;
            let new_file = self.block(&pairing.new.block_id)?.file;
            let old_path = self.patch.files.get(old_file).map(old_path);
            let new_path = self.patch.files.get(new_file).map(|file| file.path.clone());
            let mut lines = Vec::with_capacity(old.len() + new.len());
            for patch_line in old.into_iter().chain(new) {
                lines.push(section_line(&self.patch, &raw, patch_line));
            }
            sections.push(ReviewSection {
                id: format!("pair_{index}"),
                label: pairing.label.clone(),
                old_path,
                new_path,
                is_reassigned: true,
                lines,
            });
        }

        sections.extend(self.residual_sections(&raw, &assigned));
        let review = CustomReview {
            snapshot: self.clone(),
            pairings: pairings.to_vec(),
            sections,
        };
        review.validate_coverage()?;
        Ok(review)
    }

    fn block(&self, id: &str) -> Result<&ChangeBlock> {
        self.blocks
            .iter()
            .find(|block| block.id == id)
            .with_context(|| format!("Unknown change block '{id}'"))
    }

    fn select(
        &self,
        reference: &ChangeRef,
        expected: PatchLineKind,
        assigned: &mut [bool],
    ) -> Result<Vec<usize>> {
        let block = self.block(&reference.block_id)?;
        ensure!(
            block.kind == expected,
            "Block '{}' is on the wrong side of the pairing",
            reference.block_id
        );
        let offset = reference.offset.unwrap_or(0);
        ensure!(
            offset < block.line_count,
            "Offset exceeds block '{}'; its length is {}",
            block.id,
            block.line_count
        );
        let count = reference.count.unwrap_or(block.line_count - offset);
        ensure!(count > 0, "Pairing range cannot be empty");
        ensure!(
            count <= block.line_count - offset,
            "Range exceeds block '{}'; its length is {}",
            block.id,
            block.line_count
        );
        let start = block.patch_line_start + offset;
        let end = start + count;
        for slot in &mut assigned[start..end] {
            if *slot {
                bail!("Pairing reuses a changed line from block '{}'", block.id);
            }
            *slot = true;
        }
        Ok((start..end).collect())
    }

    fn residual_sections(&self, raw: &[&str], assigned: &[bool]) -> Vec<ReviewSection> {
        let mut sections = Vec::new();
        let mut by_file = vec![Vec::new(); self.patch.files.len()];
        for (index, info) in self.patch.lines.iter().enumerate() {
            if let Some(file) = info.file {
                by_file[file].push(index);
            }
        }
        for (file_index, file) in self.patch.files.iter().enumerate() {
            let mut file_sections = Vec::new();
            let mut lines = Vec::new();
            let mut section_index = 0;
            let mut seen_hunk = false;
            for &patch_line in &by_file[file_index] {
                if assigned[patch_line] {
                    continue;
                }
                let info = &self.patch.lines[patch_line];
                if info.kind == PatchLineKind::HunkHeader && seen_hunk {
                    append_residual(
                        &mut file_sections,
                        file,
                        file_index,
                        section_index,
                        &mut lines,
                    );
                    section_index += 1;
                }
                seen_hunk |= info.kind == PatchLineKind::HunkHeader;
                lines.push(section_line(&self.patch, raw, patch_line));
            }
            let has_file_metadata = file.binary
                || file.status != "modified"
                || file.old_path.is_some()
                || (file.additions == 0 && file.deletions == 0)
                || by_file[file_index].iter().any(|&index| {
                    let kind = self.patch.lines[index].kind;
                    kind == PatchLineKind::Meta
                        || (kind == PatchLineKind::FileHeader && !is_plain_file_header(raw[index]))
                });
            append_residual(
                &mut file_sections,
                file,
                file_index,
                section_index,
                &mut lines,
            );
            if !has_file_metadata {
                file_sections.retain(|section| {
                    section.lines.iter().any(|line| {
                        matches!(line.kind, PatchLineKind::Added | PatchLineKind::Removed)
                    })
                });
            }
            sections.extend(file_sections);
        }
        sections
    }
}

fn is_plain_file_header(line: &str) -> bool {
    line.starts_with("diff --git ")
        || line.starts_with("index ")
        || line.starts_with("--- ")
        || line.starts_with("+++ ")
}

impl CustomReview {
    /// Check that rendered changed lines retain their canonical identity and content.
    pub fn validate_coverage(&self) -> Result<()> {
        let mut counts = vec![0u8; self.snapshot.patch.lines.len()];
        let raw: Vec<&str> = self.snapshot.patch.text.lines().collect();
        for section in &self.sections {
            for line in &section.lines {
                let source = self
                    .snapshot
                    .patch
                    .lines
                    .get(line.source_patch_line)
                    .context("Review section references a missing patch line")?;
                ensure!(
                    source.kind == line.kind
                        && raw.get(line.source_patch_line).is_some()
                        && *line
                            == section_line(&self.snapshot.patch, &raw, line.source_patch_line),
                    "Review section changed a canonical patch line"
                );
                if matches!(line.kind, PatchLineKind::Added | PatchLineKind::Removed) {
                    counts[line.source_patch_line] =
                        counts[line.source_patch_line].saturating_add(1);
                }
            }
        }
        for (index, line) in self.snapshot.patch.lines.iter().enumerate() {
            if matches!(line.kind, PatchLineKind::Added | PatchLineKind::Removed) {
                ensure!(
                    counts[index] == 1,
                    "Changed patch line {index} appears {} times in the custom review",
                    counts[index]
                );
            }
        }
        Ok(())
    }
}

fn old_path(file: &DiffFile) -> String {
    file.old_path.clone().unwrap_or_else(|| file.path.clone())
}

fn section_line(patch: &ReviewPatch, raw: &[&str], index: usize) -> ReviewSectionLine {
    let source = patch.lines[index];
    let text = raw[index];
    let text = if matches!(
        source.kind,
        PatchLineKind::Added | PatchLineKind::Removed | PatchLineKind::Context
    ) {
        text.get(1..).unwrap_or("")
    } else {
        text
    };
    ReviewSectionLine {
        kind: source.kind,
        text: text.to_string(),
        old_line: source.old_line,
        new_line: source.new_line,
        source_patch_line: index,
    }
}

fn append_residual(
    sections: &mut Vec<ReviewSection>,
    file: &DiffFile,
    file_index: usize,
    section_index: usize,
    lines: &mut Vec<ReviewSectionLine>,
) {
    if lines.is_empty() {
        return;
    }
    sections.push(ReviewSection {
        id: format!("residual_{file_index}_{section_index}"),
        label: None,
        old_path: Some(old_path(file)),
        new_path: Some(file.path.clone()),
        is_reassigned: false,
        lines: std::mem::take(lines),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_diff::{BaseKind, PatchLine};
    use git2::{IndexAddOption, Repository, Signature};
    use std::fs;
    use std::path::PathBuf;

    fn fixture() -> ReviewPatch {
        let entries = [
            (
                "diff --git a/old.rs b/old.rs",
                PatchLineKind::FileHeader,
                0,
                None,
                None,
            ),
            (
                "@@ -1,2 +0,0 @@",
                PatchLineKind::HunkHeader,
                0,
                Some(1),
                Some(1),
            ),
            ("-alpha", PatchLineKind::Removed, 0, Some(1), Some(1)),
            ("-beta", PatchLineKind::Removed, 0, Some(2), Some(1)),
            (
                "diff --git a/new.rs b/new.rs",
                PatchLineKind::FileHeader,
                1,
                None,
                None,
            ),
            (
                "@@ -0,0 +1,2 @@",
                PatchLineKind::HunkHeader,
                1,
                Some(0),
                Some(1),
            ),
            ("+alpha", PatchLineKind::Added, 1, None, Some(1)),
            ("+beta", PatchLineKind::Added, 1, None, Some(2)),
        ];
        ReviewPatch {
            root: PathBuf::from("/tmp/review"),
            head: "feature".into(),
            base: ReviewBase {
                name: "main".into(),
                spec: "main...WORKTREE".into(),
                kind: BaseKind::Explicit,
                remote: None,
                fetched_ago: None,
                ever_fetched: false,
            },
            merge_base: None,
            comparison_base_oid: "0123456789abcdef0123456789abcdef01234567".into(),
            ahead: 0,
            behind: 0,
            files: vec![
                DiffFile {
                    path: "old.rs".into(),
                    old_path: None,
                    status: "modified".into(),
                    additions: 0,
                    deletions: 2,
                    binary: false,
                },
                DiffFile {
                    path: "new.rs".into(),
                    old_path: None,
                    status: "added".into(),
                    additions: 2,
                    deletions: 0,
                    binary: false,
                },
            ],
            text: entries
                .iter()
                .map(|entry| format!("{}\n", entry.0))
                .collect(),
            lines: entries
                .iter()
                .map(|(_, kind, file, old_line, new_line)| PatchLine {
                    kind: *kind,
                    file: Some(*file),
                    old_line: *old_line,
                    new_line: *new_line,
                })
                .collect(),
            truncated: false,
        }
    }

    fn reference(id: &str, offset: usize, count: usize) -> ChangeRef {
        ChangeRef {
            block_id: id.into(),
            offset: Some(offset),
            count: Some(count),
        }
    }

    #[test]
    fn cross_file_reassignment_preserves_every_changed_line() {
        let snapshot = ReviewSnapshot::from_patch(fixture()).unwrap();
        assert_eq!(snapshot.blocks.len(), 2);
        let pair = DiffPairing {
            label: Some("Moved alpha".into()),
            old: reference(&snapshot.blocks[0].id, 0, 1),
            new: reference(&snapshot.blocks[1].id, 0, 1),
        };
        let review = snapshot.reassign(&[pair]).unwrap();
        assert_eq!(review.sections[0].old_path.as_deref(), Some("old.rs"));
        assert_eq!(review.sections[0].new_path.as_deref(), Some("new.rs"));
        assert_eq!(review.sections[0].lines[0].source_patch_line, 2);
        assert_eq!(review.sections[0].lines[1].source_patch_line, 6);
        assert!(review
            .sections
            .iter()
            .any(|section| { section.lines.iter().any(|line| line.source_patch_line == 3) }));
        assert!(review
            .sections
            .iter()
            .any(|section| { section.lines.iter().any(|line| line.source_patch_line == 7) }));
        review.validate_coverage().unwrap();
        let replay: CustomReview =
            serde_json::from_str(&serde_json::to_string(&review).unwrap()).unwrap();
        assert_eq!(replay, review);
        replay.validate_coverage().unwrap();
    }

    #[test]
    fn rejects_overlapping_and_invalid_pairings() {
        let snapshot = ReviewSnapshot::from_patch(fixture()).unwrap();
        let old = &snapshot.blocks[0].id;
        let new = &snapshot.blocks[1].id;
        let pair = DiffPairing {
            label: None,
            old: reference(old, 0, 1),
            new: reference(new, 0, 1),
        };
        assert!(snapshot.reassign(&[pair.clone(), pair]).is_err());
        assert!(snapshot
            .reassign(&[DiffPairing {
                label: None,
                old: reference(new, 0, 1),
                new: reference(old, 0, 1),
            }])
            .is_err());
        assert!(snapshot
            .reassign(&[DiffPairing {
                label: None,
                old: reference(old, 1, 2),
                new: reference(new, 0, 1),
            }])
            .is_err());
    }

    #[test]
    fn refuses_incomplete_snapshots() {
        let mut patch = fixture();
        patch.truncated = true;
        assert!(ReviewSnapshot::from_patch(patch).is_err());
        let mut patch = fixture();
        patch.lines.pop();
        assert!(ReviewSnapshot::from_patch(patch).is_err());
        let mut patch = fixture();
        patch.text = patch.text.replacen(
            "diff --git a/old.rs b/old.rs",
            &format!("diff --git {}", "x".repeat(super::super::MAX_PATCH_BYTES)),
            1,
        );
        assert!(ReviewSnapshot::from_patch(patch).is_err());
    }

    #[test]
    fn retains_mode_metadata_after_all_text_is_reassigned() {
        let mut patch = fixture();
        patch.text = patch.text.replacen(
            "diff --git a/old.rs b/old.rs\n",
            "diff --git a/old.rs b/old.rs\nold mode 100644\n",
            1,
        );
        patch.lines.insert(
            1,
            PatchLine {
                kind: PatchLineKind::FileHeader,
                file: Some(0),
                old_line: None,
                new_line: None,
            },
        );
        let snapshot = ReviewSnapshot::from_patch(patch).unwrap();
        let review = snapshot
            .reassign(&[DiffPairing {
                label: None,
                old: reference(&snapshot.blocks[0].id, 0, 2),
                new: reference(&snapshot.blocks[1].id, 0, 2),
            }])
            .unwrap();
        assert!(review.sections.iter().any(|section| {
            section
                .lines
                .iter()
                .any(|line| line.text == "old mode 100644")
        }));
    }

    #[test]
    fn snapshot_matches_git_review_with_staged_unstaged_and_untracked_moves() {
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();
        repo.set_head("refs/heads/main").unwrap();
        let unchanged = (0..20)
            .map(|number| format!("retained {number}\n"))
            .collect::<String>();
        fs::write(
            temp.path().join("old.rs"),
            format!("alpha\nbeta\n{unchanged}"),
        )
        .unwrap();
        let mut index = repo.index().unwrap();
        index.add_all(["*"], IndexAddOption::DEFAULT, None).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let signature = Signature::now("Ovim", "ovim@example.com").unwrap();
        repo.commit(Some("HEAD"), &signature, &signature, "initial", &tree, &[])
            .unwrap();
        drop(tree);

        fs::write(temp.path().join("old.rs"), &unchanged).unwrap();
        fs::write(temp.path().join("new.rs"), "alpha refactored\n").unwrap();
        fs::write(temp.path().join("staged.rs"), "staged change\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("new.rs")).unwrap();
        index.add_path(Path::new("staged.rs")).unwrap();
        index.write().unwrap();
        fs::write(temp.path().join("new.rs"), "alpha refactored\nbeta moved\n").unwrap();
        fs::write(temp.path().join("untracked.rs"), "untracked change\n").unwrap();

        let base = super::super::resolve_pullbase(temp.path(), Some("main")).unwrap();
        let regular = review_patch(temp.path(), &base).unwrap();
        let snapshot = review_snapshot(temp.path(), &base).unwrap();
        assert_eq!(snapshot.patch, regular);
        assert_eq!(snapshot.patch.comparison_base_oid.len(), 40);
        for name in ["old.rs", "new.rs", "staged.rs", "untracked.rs"] {
            assert!(snapshot.patch.files.iter().any(|file| file.path == name));
        }
        let old = snapshot
            .blocks
            .iter()
            .find(|block| {
                block.kind == PatchLineKind::Removed
                    && snapshot.patch.files[block.file].path == "old.rs"
            })
            .unwrap();
        let new = snapshot
            .blocks
            .iter()
            .find(|block| {
                block.kind == PatchLineKind::Added
                    && snapshot.patch.files[block.file].path == "new.rs"
            })
            .unwrap();
        let review = snapshot
            .reassign(&[DiffPairing {
                label: Some("Move and edit".into()),
                old: reference(&old.id, 0, old.line_count),
                new: reference(&new.id, 0, new.line_count),
            }])
            .unwrap();
        review.validate_coverage().unwrap();
        assert_eq!(review.sections[0].old_path.as_deref(), Some("old.rs"));
        assert_eq!(review.sections[0].new_path.as_deref(), Some("new.rs"));
        assert_eq!(review.snapshot.patch.additions(), regular.additions());
        assert_eq!(review.snapshot.patch.deletions(), regular.deletions());
    }
}
