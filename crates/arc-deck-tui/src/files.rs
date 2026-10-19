//! Workspace file listing for `@` references.
//!
//! Source of truth is `git ls-files --cached --others --exclude-standard`: tracked plus
//! untracked-but-not-ignored files, which is exactly the set a developer means by "the files
//! in this repo". `target/` and friends never appear because git ignores them; a plain
//! directory walk would list tens of thousands of build artifacts.
//!
//! Not a git repository → bounded walk that skips the same noise directories. The walk has hard
//! limits because `@` must stay responsive in a directory like `C:\`.
//!
//! Windows child processes here are started with `CREATE_NO_WINDOW` (AGENTS b227176: a flashing
//! console window is a defect, not a detail).

use std::path::Path;
use std::process::{Command, Stdio};

use crate::autocomplete::fuzzy;

const WALK_MAX_FILES: usize = 20_000;
const WALK_MAX_DEPTH: usize = 8;
/// Directories whose contents are build output or metadata. Matched by name at any depth.
const SKIP_DIRS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    "dist",
    "build",
    ".idea",
    ".vs",
    ".next",
    "__pycache__",
    ".venv",
];

#[derive(Debug, Default)]
pub(crate) struct FileIndex {
    files: Vec<String>,
    loaded: bool,
}

impl FileIndex {
    /// Lazily fills the list. Called on the first `@`; failure is not an error — an empty list
    /// simply means the popup has nothing to offer, and the user can still type the path.
    pub(crate) fn ensure(&mut self, workspace: &Path) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        self.files = list(workspace);
    }

    #[cfg(test)]
    pub(crate) fn files(&self) -> &[String] {
        &self.files
    }

    /// Candidates for the text after `@`. `recent` is most-recently-used first and adds a
    /// bounded bonus, so a file used a minute ago wins over an equally good fuzzy match.
    /// An empty query lists recent files first, then everything else in path order.
    pub(crate) fn matches(&self, query: &str, recent: &[String], limit: usize) -> Vec<String> {
        let mut scored: Vec<(i32, usize, String)> = Vec::new();
        for (order, path) in self.files.iter().enumerate() {
            let Some(score) = fuzzy(query, path) else {
                continue;
            };
            // +8 for the most recent, decaying by one per position, floor 1.
            let bonus = match recent.iter().position(|item| item == path) {
                Some(index) => 8i32.saturating_sub(index as i32).max(1),
                None => 0,
            };
            scored.push((score + bonus, order, path.clone()));
        }
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        scored
            .into_iter()
            .take(limit)
            .map(|(_, _, path)| path)
            .collect()
    }
}

/// Lists files relative to `workspace`, `/`-separated. Never fails: the fallback is an empty
/// list, and the popup then shows nothing.
pub(crate) fn list(workspace: &Path) -> Vec<String> {
    if let Some(files) = git_files(workspace) {
        return files;
    }
    walk(workspace)
}

fn git_files(workspace: &Path) -> Option<Vec<String>> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(workspace)
        .args(["ls-files", "--cached", "--others", "--exclude-standard", "-z"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let mut files: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(|path| path.replace('\\', "/"))
        .collect();
    files.sort_unstable();
    files.dedup();
    Some(files)
}

fn walk(workspace: &Path) -> Vec<String> {
    let mut files = Vec::new();
    let mut stack = vec![(workspace.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > WALK_MAX_DEPTH || files.len() >= WALK_MAX_FILES {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.is_dir() {
                if !SKIP_DIRS.contains(&name.as_ref()) {
                    stack.push((path, depth + 1));
                }
            } else if metadata.is_file() {
                if let Ok(relative) = path.strip_prefix(workspace) {
                    files.push(relative.to_string_lossy().replace('\\', "/"));
                }
                if files.len() >= WALK_MAX_FILES {
                    return files;
                }
            }
        }
    }
    files.sort_unstable();
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(root: &Path, relative: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "x").unwrap();
    }

    #[test]
    fn the_walk_skips_build_and_metadata_directories() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "src/main.rs");
        touch(dir.path(), "README.md");
        touch(dir.path(), "target/debug/build/artifact.bin");
        touch(dir.path(), "node_modules/pkg/index.js");
        touch(dir.path(), ".git/config");
        let files = walk(dir.path());
        assert_eq!(files, vec!["README.md".to_string(), "src/main.rs".to_string()]);
    }

    #[test]
    fn fuzzy_matching_ranks_recent_files_first() {
        let index = FileIndex {
            files: vec![
                "src/main.rs".into(),
                "src/markdown.rs".into(),
                "docs/markdown.md".into(),
            ],
            loaded: true,
        };
        let recent = vec!["docs/markdown.md".to_string()];
        let matches = index.matches("markdown", &recent, 10);
        assert_eq!(matches[0], "docs/markdown.md");
        assert!(matches.contains(&"src/markdown.rs".to_string()));
        assert!(!matches.contains(&"src/main.rs".to_string()));
    }

    #[test]
    fn an_empty_query_lists_recent_then_files_in_path_order() {
        let index = FileIndex {
            files: vec!["a.rs".into(), "b.rs".into(), "c.rs".into()],
            loaded: true,
        };
        let recent = vec!["c.rs".to_string()];
        let matches = index.matches("", &recent, 10);
        assert_eq!(matches, vec!["c.rs", "a.rs", "b.rs"]);
    }

    #[test]
    fn candidates_are_capped_so_a_huge_repo_stays_responsive() {
        let index = FileIndex {
            files: (0..500).map(|i| format!("file{i:03}.rs")).collect(),
            loaded: true,
        };
        assert_eq!(index.matches("file", &[], 12).len(), 12);
    }

    #[test]
    fn ensure_loads_only_once() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "one.txt");
        let mut index = FileIndex::default();
        index.ensure(dir.path());
        assert_eq!(index.files(), ["one.txt".to_string()]);
        touch(dir.path(), "two.txt");
        index.ensure(dir.path());
        assert_eq!(index.files(), ["one.txt".to_string()], "no rescans per keystroke");
    }
}
