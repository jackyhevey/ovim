use std::fs;
use std::path::{Path, PathBuf};

fn collect_rs_files(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

#[test]
fn test_pending_semantic_change_apis_are_removed() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("ovim crate should live under repo root");
    let core_src = repo_root.join("ovim-core").join("src");

    let mut files = Vec::new();
    collect_rs_files(&core_src, &mut files);

    let mut legacy_hits = Vec::new();

    for path in files {
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        let relative = path
            .strip_prefix(repo_root)
            .expect("core source file should be under repo root")
            .to_string_lossy()
            .replace('\\', "/");

        for (line_number, line) in content.lines().enumerate() {
            if line.contains("set_pending_semantic_change(")
                || line.contains("take_pending_semantic_change(")
                || line.contains("pending_semantic_change")
                || line.contains("PendingSemanticChange")
            {
                legacy_hits.push(format!("{}:{}", relative, line_number + 1));
            }
        }
    }

    assert!(
        legacy_hits.is_empty(),
        "Legacy pending semantic-change APIs/state should be removed, found:\n{}",
        legacy_hits.join("\n")
    );
}
