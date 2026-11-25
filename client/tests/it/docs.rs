//! The repository's own documentation, checked against the tree.

use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// DOC-13: `architecture.md`'s code map — the canonical "where is X" page —
/// pointed at files a refactor had split away, and nothing noticed. Every
/// repository path a doc quotes in backticks must exist.
///
/// A path counts when it has a `/` and ends in an extension or a `/`, which
/// leaves out branch names (`fix/ipv6-parse`). It resolves against the
/// repository root, the document's own folder, or `desktop/` (the frontend
/// docs write `lib/bridge.js`). The audit reports are skipped — they describe
/// the tree as it was — as is any line that says the path was deleted.
#[test]
fn every_path_the_docs_name_exists() {
    let root = repo();
    let mut docs: Vec<PathBuf> = std::fs::read_dir(root.join("docs"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .collect();
    for top in [
        "README.md",
        "SECURITY.md",
        "CLAUDE.md",
        "CONTRIBUTING.md",
        "THREAT_MODEL.md",
    ] {
        if root.join(top).exists() {
            docs.push(root.join(top));
        }
    }
    let mut missing = Vec::new();
    for doc in &docs {
        let text = std::fs::read_to_string(doc).unwrap();
        let base = doc.parent().unwrap();
        for (n, line) in text.lines().enumerate() {
            let lower = line.to_ascii_lowercase();
            if [
                "delet",
                "removed",
                "retired",
                "was split",
                "no longer exists",
            ]
            .iter()
            .any(|w| lower.contains(w))
            {
                continue;
            }
            for quoted in line.split('`').skip(1).step_by(2) {
                let path = quoted.split(':').next().unwrap_or_default();
                let is_path = path.trim_end_matches('/').contains('/')
                    && !path.contains(['*', '{', '}', ' ', '<', '$'])
                    && !path.starts_with(['.', '/', '~'])
                    && !path.contains("://")
                    && (path.ends_with('/')
                        || Path::new(path).extension().is_some_and(|e| e.len() <= 5));
                if !is_path {
                    continue;
                }
                let found = [
                    root.clone(),
                    base.to_path_buf(),
                    root.join("desktop"),
                    root.join("desktop/src"),
                ]
                .iter()
                .any(|dir| dir.join(path).exists());
                if !found {
                    missing.push(format!(
                        "{}:{}: `{path}`",
                        doc.strip_prefix(&root).unwrap_or(doc).display(),
                        n + 1
                    ));
                }
            }
        }
    }
    assert!(
        missing.is_empty(),
        "docs name paths that do not exist:\n{}",
        missing.join("\n")
    );
}
