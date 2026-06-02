use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use walkdir::{DirEntry, WalkDir};

use crate::github::GitHubIssue;
use crate::scoring::normalize;

const MAX_DISCOVERED_FILES: usize = 500;
const MAX_FILE_READ_BYTES: u64 = 64 * 1024;
const MAX_SNIPPET_CHARS: usize = 1_200;

const EXCLUDED_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    "vendor",
    "coverage",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepoScan {
    pub discovered_files: Vec<String>,
    pub candidate_files: Vec<CandidateFile>,
    pub validation_commands: Vec<ValidationCommand>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CandidateFile {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ValidationCommand {
    pub command: String,
    pub reason: String,
}

pub fn scan_repository(root: &Path, issue: &GitHubIssue) -> RepoScan {
    let mut warnings = Vec::new();
    let discovered_files = discover_files(root, &mut warnings);
    let candidate_files = detect_candidate_files(root, issue, &discovered_files);
    let validation_commands = detect_validation_commands(root);

    RepoScan {
        discovered_files,
        candidate_files,
        validation_commands,
        warnings,
    }
}

pub fn discover_files(root: &Path, warnings: &mut Vec<String>) -> Vec<String> {
    let mut files = Vec::new();

    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !is_excluded_entry(entry))
    {
        let Ok(entry) = entry else {
            warnings.push("Unable to read one repository entry during scan".to_string());
            continue;
        };

        if !entry.file_type().is_file() {
            continue;
        }

        let Ok(relative) = entry.path().strip_prefix(root) else {
            continue;
        };

        files.push(relative.to_string_lossy().replace('\\', "/"));
        if files.len() >= MAX_DISCOVERED_FILES {
            warnings.push(format!(
                "Repository scan stopped after {MAX_DISCOVERED_FILES} discovered files"
            ));
            break;
        }
    }

    files
}

pub fn detect_candidate_files(
    root: &Path,
    issue: &GitHubIssue,
    discovered_files: &[String],
) -> Vec<CandidateFile> {
    let issue_text = format!("{}\n{}", issue.title, issue.body);
    let terms = issue_terms(&issue_text);
    let referenced_paths = extract_referenced_paths(&issue_text);

    let mut scored = discovered_files
        .iter()
        .filter_map(|relative| {
            let path_score = score_path(relative, &terms, &referenced_paths);
            let snippet_score = score_snippet(&root.join(relative), &terms);
            let score = path_score + snippet_score;
            if score <= 0 {
                return None;
            }

            let reason = if referenced_paths.iter().any(|path| relative.ends_with(path)) {
                "Issue body referenced this path".to_string()
            } else if path_score > 0 {
                "Path matched issue terms".to_string()
            } else {
                "File snippet matched issue terms".to_string()
            };

            Some((
                score,
                CandidateFile {
                    path: relative.clone(),
                    reason,
                },
            ))
        })
        .collect::<Vec<_>>();

    scored.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| left.1.path.cmp(&right.1.path))
    });
    scored
        .into_iter()
        .map(|(_, candidate)| candidate)
        .take(8)
        .collect()
}

pub fn detect_validation_commands(root: &Path) -> Vec<ValidationCommand> {
    let mut commands = Vec::new();

    if root.join("Cargo.toml").exists() {
        commands.push(ValidationCommand {
            command: "cargo test".to_string(),
            reason: "Detected Cargo.toml".to_string(),
        });
    }

    if root.join("package.json").exists() {
        commands.push(ValidationCommand {
            command: detect_package_test_command(root),
            reason: "Detected package.json".to_string(),
        });
    }

    if root.join("pyproject.toml").exists() {
        commands.push(ValidationCommand {
            command: "pytest".to_string(),
            reason: "Detected pyproject.toml".to_string(),
        });
    }

    if root.join("go.mod").exists() {
        commands.push(ValidationCommand {
            command: "go test ./...".to_string(),
            reason: "Detected go.mod".to_string(),
        });
    }

    if makefile_has_test_target(root) {
        commands.push(ValidationCommand {
            command: "make test".to_string(),
            reason: "Detected Makefile test target".to_string(),
        });
    }

    dedupe_commands(commands)
}

fn is_excluded_entry(entry: &DirEntry) -> bool {
    if !entry.file_type().is_dir() {
        return false;
    }

    entry
        .file_name()
        .to_str()
        .map(|name| EXCLUDED_DIRS.contains(&name))
        .unwrap_or(false)
}

fn issue_terms(text: &str) -> Vec<String> {
    let mut terms = HashSet::new();
    for term in normalize(text).split_whitespace() {
        if term.len() >= 3 && !is_stop_word(term) {
            terms.insert(term.to_string());
        }
    }
    let mut values = terms.into_iter().collect::<Vec<_>>();
    values.sort();
    values
}

fn is_stop_word(term: &str) -> bool {
    matches!(
        term,
        "the"
            | "and"
            | "for"
            | "with"
            | "from"
            | "this"
            | "that"
            | "into"
            | "issue"
            | "bug"
            | "fix"
            | "add"
    )
}

fn extract_referenced_paths(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|token| {
            token.trim_matches(|ch: char| {
                !ch.is_ascii_alphanumeric() && ch != '/' && ch != '.' && ch != '-' && ch != '_'
            })
        })
        .filter(|token| token.contains('/'))
        .filter(|token| {
            [
                ".rs", ".ts", ".tsx", ".js", ".jsx", ".py", ".go", ".java", ".kt", ".json", ".md",
                ".css", ".scss",
            ]
            .iter()
            .any(|suffix| token.ends_with(suffix))
        })
        .map(ToOwned::to_owned)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect()
}

fn score_path(path: &str, terms: &[String], referenced_paths: &[String]) -> i32 {
    let normalized_path = normalize(path);
    let mut score = 0;

    for referenced_path in referenced_paths {
        if path.ends_with(referenced_path) {
            score += 60;
        } else if path.contains(referenced_path) || file_name(path) == file_name(referenced_path) {
            score += 30;
        }
    }

    for term in terms {
        if normalized_path.contains(term) {
            score += 5;
        }
    }

    if matches!(
        Path::new(path).extension().and_then(|value| value.to_str()),
        Some("rs" | "ts" | "tsx" | "js" | "jsx" | "py" | "go" | "md")
    ) {
        score += 2;
    }

    score
}

fn score_snippet(path: &Path, terms: &[String]) -> i32 {
    let Ok(metadata) = fs::metadata(path) else {
        return 0;
    };
    if metadata.len() > MAX_FILE_READ_BYTES {
        return 0;
    }

    let Ok(bytes) = fs::read(path) else {
        return 0;
    };
    if bytes.contains(&0) {
        return 0;
    }

    let content = String::from_utf8_lossy(&bytes);
    let snippet = content.chars().take(MAX_SNIPPET_CHARS).collect::<String>();
    let normalized = normalize(&snippet);
    terms
        .iter()
        .filter(|term| normalized.contains(term.as_str()))
        .count() as i32
}

fn detect_package_test_command(root: &Path) -> String {
    let package_manager = fs::read_to_string(root.join("package.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|json| {
            json.get("packageManager")
                .and_then(|value| value.as_str())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_default();

    if package_manager.starts_with("bun@")
        || root.join("bun.lock").exists()
        || root.join("bun.lockb").exists()
    {
        return "bun test".to_string();
    }
    if package_manager.starts_with("pnpm@") || root.join("pnpm-lock.yaml").exists() {
        return "pnpm test".to_string();
    }
    if package_manager.starts_with("yarn@") || root.join("yarn.lock").exists() {
        return "yarn test".to_string();
    }
    "npm test".to_string()
}

fn makefile_has_test_target(root: &Path) -> bool {
    for name in ["Makefile", "makefile"] {
        let path = root.join(name);
        let Ok(raw) = fs::read_to_string(path) else {
            continue;
        };
        if raw
            .lines()
            .any(|line| line.trim_start().starts_with("test:"))
        {
            return true;
        }
    }
    false
}

fn dedupe_commands(commands: Vec<ValidationCommand>) -> Vec<ValidationCommand> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();
    for command in commands {
        if seen.insert(command.command.clone()) {
            deduped.push(command);
        }
    }
    deduped
}

fn file_name(path: &str) -> String {
    PathBuf::from(path)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(path)
        .to_string()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use chrono::Utc;
    use tempfile::tempdir;

    use super::{detect_validation_commands, discover_files, scan_repository};
    use crate::github::GitHubIssue;

    fn issue() -> GitHubIssue {
        GitHubIssue {
            id: 1,
            number: 7,
            title: "Fix parser panic".to_string(),
            body: "The failure is in src/parser.rs. Expected graceful error.".to_string(),
            labels: vec!["good first issue".to_string()],
            url: "https://github.com/owner/repo/issues/7".to_string(),
            repo_full_name: "owner/repo".to_string(),
            repo_name: "repo".to_string(),
            repo_description: String::new(),
            repo_stars: 0,
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
        }
    }

    #[test]
    fn excludes_heavy_directories() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::create_dir_all(dir.path().join("node_modules/pkg")).unwrap();
        fs::write(dir.path().join("src/lib.rs"), "").unwrap();
        fs::write(dir.path().join("node_modules/pkg/index.js"), "").unwrap();

        let mut warnings = Vec::new();
        let files = discover_files(dir.path(), &mut warnings);
        assert_eq!(files, vec!["src/lib.rs"]);
    }

    #[test]
    fn detects_validation_commands_without_running_them() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"x\"").unwrap();
        fs::write(dir.path().join("package.json"), "{}").unwrap();
        fs::write(dir.path().join("pnpm-lock.yaml"), "").unwrap();
        fs::write(dir.path().join("Makefile"), "test:\n\ttrue\n").unwrap();

        let commands = detect_validation_commands(dir.path())
            .into_iter()
            .map(|item| item.command)
            .collect::<Vec<_>>();
        assert!(commands.contains(&"cargo test".to_string()));
        assert!(commands.contains(&"pnpm test".to_string()));
        assert!(commands.contains(&"make test".to_string()));
    }

    #[test]
    fn selects_candidate_file_from_referenced_path() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/parser.rs"), "fn parse() {}").unwrap();
        let scan = scan_repository(dir.path(), &issue());
        assert_eq!(scan.candidate_files[0].path, "src/parser.rs");
    }
}
