use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::Value;
use walkdir::WalkDir;

const OUTPUT_KIND: &str = "issue_finder_profile_bootstrap_report";
const SCAN_DEPTH: &str = "root_manifest_only";
const CONVERSATION_BODY_MODE: &str = "disabled";

const MANIFEST_FILES: &[&str] = &[
    "Cargo.toml",
    "package.json",
    "go.mod",
    "pyproject.toml",
    "requirements.txt",
    "pom.xml",
    "build.gradle",
    "settings.gradle",
    "Gemfile",
    "composer.json",
];

const DANGEROUS_JSON_KEYS: &[&str] = &[
    "assistant",
    "body",
    "content",
    "diff",
    "message",
    "messages",
    "output",
    "patch",
    "prompt",
    "response",
    "result",
    "stderr",
    "stdout",
    "system",
    "tool",
    "tool_output",
    "transcript",
];

const SAFE_TEXT_KEYS: &[&str] = &[
    "headline",
    "summary",
    "task",
    "theme",
    "thread_name",
    "title",
    "topic",
];

const PATH_KEYS: &[&str] = &[
    "cwd",
    "current_directory",
    "current_working_directory",
    "directory",
    "project_path",
    "projectpath",
    "repo_path",
    "repository_path",
    "root",
    "workspace",
    "workspace_path",
    "workspacepath",
    "working_directory",
];

const TIMESTAMP_KEYS: &[&str] = &[
    "created_at",
    "last_active_at",
    "last_seen_at",
    "started_at",
    "start_time",
    "time",
    "timestamp",
    "updated_at",
];

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileBootstrapReport {
    pub kind: String,
    pub version: u8,
    pub scan_scope: ScanScope,
    pub agent_sources: Vec<AgentSourceReport>,
    pub active_projects: Vec<ActiveProject>,
    pub tech_stack_evidence: Vec<EvidenceOutput>,
    pub keyword_evidence: Vec<EvidenceOutput>,
    pub recent_task_themes: Vec<RecentTaskTheme>,
    pub recommended_profile: RecommendedProfile,
    pub warnings: Vec<BootstrapWarning>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScanScope {
    pub agent_sources: Vec<String>,
    pub scan_depth: String,
    pub full_supported_source_scan: bool,
    pub conversation_body_mode: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentSourceReport {
    pub kind: String,
    pub path: String,
    pub status: String,
    pub records_seen: usize,
    pub records_parsed: usize,
    pub warnings: Vec<BootstrapWarning>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActiveProject {
    pub id: String,
    pub path: String,
    pub first_seen_at: Option<String>,
    pub last_seen_at: Option<String>,
    pub session_count: usize,
    pub memory_count: usize,
    pub manifest_count: usize,
    pub sources: Vec<String>,
    pub manifests: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceOutput {
    pub term: String,
    pub weight: i32,
    pub count: usize,
    pub sources: Vec<String>,
    pub project_refs: Vec<String>,
    pub manifest_refs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RecentTaskTheme {
    pub theme: String,
    pub count: usize,
    pub sources: Vec<String>,
    pub last_seen_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RecommendedProfile {
    pub tech_stack: Vec<String>,
    pub keywords: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapWarning {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone)]
struct AgentRecord {
    source_ref: String,
    record_kind: RecordKind,
    cwd_paths: Vec<PathBuf>,
    safe_text_fragments: Vec<String>,
    timestamp: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordKind {
    Session,
    Memory,
}

#[derive(Debug, Default)]
struct JsonRecordCollector {
    cwd_paths: Vec<PathBuf>,
    safe_text_fragments: Vec<String>,
    timestamp: Option<String>,
}

#[derive(Debug, Default)]
struct ProjectBuilder {
    path: PathBuf,
    first_seen_at: Option<String>,
    last_seen_at: Option<String>,
    session_count: usize,
    memory_count: usize,
    sources: BTreeSet<String>,
    safe_texts: Vec<SourceText>,
    manifests: Vec<ManifestEvidence>,
}

#[derive(Debug, Clone)]
struct SourceText {
    source_ref: String,
    text: String,
    timestamp: Option<String>,
}

#[derive(Debug, Clone)]
struct ManifestEvidence {
    path: PathBuf,
    file_name: String,
    tech_terms: Vec<String>,
    keyword_terms: Vec<String>,
}

#[derive(Debug, Default)]
struct EvidenceBuilder {
    weight: i32,
    count: usize,
    sources: BTreeSet<String>,
    project_refs: BTreeSet<String>,
    manifest_refs: BTreeSet<String>,
    reasons: BTreeSet<String>,
}

#[derive(Debug, Default)]
struct ThemeBuilder {
    count: usize,
    sources: BTreeSet<String>,
    last_seen_at: Option<String>,
}

pub fn bootstrap_profile(scan_root: &Path) -> Result<ProfileBootstrapReport> {
    let scan_root = scan_root
        .canonicalize()
        .with_context(|| format!("unable to resolve scan root {}", scan_root.display()))?;
    let mut warnings = Vec::new();
    let (source_reports, records) = scan_agent_sources(&scan_root, &mut warnings);
    if source_reports.is_empty() {
        warnings.push(BootstrapWarning::new(
            "no_agent_sources_found",
            "No supported Codex, Claude, or Cursor profile bootstrap sources were found.",
            None,
        ));
    }

    let mut projects = build_projects(records, &mut warnings);
    scan_project_manifests(&mut projects, &mut warnings);
    filter_non_project_paths(&mut projects, &scan_root);
    if projects.is_empty() {
        warnings.push(BootstrapWarning::new(
            "no_active_projects_found",
            "No active project directories were discovered from supported Agent sources.",
            None,
        ));
    }
    let (active_projects, tech_stack_evidence, keyword_evidence, recent_task_themes) =
        build_outputs(projects);
    let recommended_profile =
        recommended_profile(&tech_stack_evidence, &keyword_evidence, &recent_task_themes);

    Ok(ProfileBootstrapReport {
        kind: OUTPUT_KIND.to_string(),
        version: 1,
        scan_scope: ScanScope {
            agent_sources: vec![
                "codex".to_string(),
                "claude".to_string(),
                "cursor".to_string(),
            ],
            scan_depth: SCAN_DEPTH.to_string(),
            full_supported_source_scan: true,
            conversation_body_mode: CONVERSATION_BODY_MODE.to_string(),
        },
        agent_sources: source_reports,
        active_projects,
        tech_stack_evidence,
        keyword_evidence,
        recent_task_themes,
        recommended_profile,
        warnings,
    })
}

pub fn render_profile_bootstrap_report(report: &ProfileBootstrapReport) -> String {
    let tech_stack = if report.recommended_profile.tech_stack.is_empty() {
        "none".to_string()
    } else {
        report.recommended_profile.tech_stack.join(", ")
    };
    let keywords = if report.recommended_profile.keywords.is_empty() {
        "none".to_string()
    } else {
        report
            .recommended_profile
            .keywords
            .iter()
            .take(12)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    };

    format!(
        "Profile bootstrap scanned {} source file(s), found {} active project(s).\nRecommended tech stack: {tech_stack}\nRecommended keywords: {keywords}\nWarnings: {}",
        report.agent_sources.len(),
        report.active_projects.len(),
        report.warnings.len()
    )
}

fn scan_agent_sources(
    scan_root: &Path,
    warnings: &mut Vec<BootstrapWarning>,
) -> (Vec<AgentSourceReport>, Vec<AgentRecord>) {
    let mut source_reports = Vec::new();
    let mut records = Vec::new();

    scan_codex_sources(scan_root, &mut source_reports, &mut records, warnings);
    scan_claude_sources(scan_root, &mut source_reports, &mut records, warnings);
    scan_cursor_sources(scan_root, &mut source_reports, &mut records, warnings);

    (source_reports, records)
}

fn scan_codex_sources(
    scan_root: &Path,
    source_reports: &mut Vec<AgentSourceReport>,
    records: &mut Vec<AgentRecord>,
    warnings: &mut Vec<BootstrapWarning>,
) {
    let codex = scan_root.join(".codex");
    scan_supported_file(
        &codex.join("session_index.jsonl"),
        "codex",
        RecordKind::Session,
        scan_root,
        source_reports,
        records,
        warnings,
    );
    scan_supported_file(
        &codex.join("history.jsonl"),
        "codex",
        RecordKind::Session,
        scan_root,
        source_reports,
        records,
        warnings,
    );
    scan_jsonl_tree(
        &codex.join("sessions"),
        "codex",
        RecordKind::Session,
        source_reports,
        records,
        warnings,
        scan_root,
    );
    scan_jsonl_tree(
        &codex.join("archived_sessions"),
        "codex",
        RecordKind::Session,
        source_reports,
        records,
        warnings,
        scan_root,
    );
    scan_memory_dir(
        &codex.join("memories"),
        "codex",
        scan_root,
        source_reports,
        records,
        warnings,
    );
}

fn scan_jsonl_tree(
    root: &Path,
    source_kind: &str,
    record_kind: RecordKind,
    source_reports: &mut Vec<AgentSourceReport>,
    records: &mut Vec<AgentRecord>,
    warnings: &mut Vec<BootstrapWarning>,
    scan_root: &Path,
) {
    if !root.exists() {
        return;
    }
    for entry in WalkDir::new(root)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
    {
        if should_skip_agent_file(entry.path()) {
            continue;
        }
        if entry
            .path()
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("jsonl"))
        {
            scan_supported_file(
                entry.path(),
                source_kind,
                record_kind,
                scan_root,
                source_reports,
                records,
                warnings,
            );
        }
    }
}

fn scan_claude_sources(
    scan_root: &Path,
    source_reports: &mut Vec<AgentSourceReport>,
    records: &mut Vec<AgentRecord>,
    warnings: &mut Vec<BootstrapWarning>,
) {
    scan_matching_tree(
        &scan_root.join(".claude"),
        "claude",
        scan_root,
        8,
        source_reports,
        records,
        warnings,
    );
}

fn scan_cursor_sources(
    scan_root: &Path,
    source_reports: &mut Vec<AgentSourceReport>,
    records: &mut Vec<AgentRecord>,
    warnings: &mut Vec<BootstrapWarning>,
) {
    scan_matching_tree(
        &scan_root.join(".cursor"),
        "cursor",
        scan_root,
        8,
        source_reports,
        records,
        warnings,
    );

    let cursor_user = scan_root.join("Library/Application Support/Cursor/User");
    scan_matching_tree(
        &cursor_user,
        "cursor",
        scan_root,
        5,
        source_reports,
        records,
        warnings,
    );
}

fn scan_memory_dir(
    dir: &Path,
    source_kind: &str,
    scan_root: &Path,
    source_reports: &mut Vec<AgentSourceReport>,
    records: &mut Vec<AgentRecord>,
    warnings: &mut Vec<BootstrapWarning>,
) {
    if !dir.exists() {
        return;
    }
    for entry in WalkDir::new(dir)
        .max_depth(6)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
    {
        if should_skip_agent_file(entry.path()) {
            continue;
        }
        scan_supported_file(
            entry.path(),
            source_kind,
            RecordKind::Memory,
            scan_root,
            source_reports,
            records,
            warnings,
        );
    }
}

fn scan_matching_tree(
    root: &Path,
    source_kind: &str,
    scan_root: &Path,
    max_depth: usize,
    source_reports: &mut Vec<AgentSourceReport>,
    records: &mut Vec<AgentRecord>,
    warnings: &mut Vec<BootstrapWarning>,
) {
    if !root.exists() {
        return;
    }

    for entry in WalkDir::new(root)
        .max_depth(max_depth)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
    {
        if should_skip_agent_file(entry.path()) {
            continue;
        }
        if !looks_like_agent_index(entry.path()) {
            continue;
        }
        let record_kind = if file_name_lower(entry.path()).contains("memor") {
            RecordKind::Memory
        } else {
            RecordKind::Session
        };
        scan_supported_file(
            entry.path(),
            source_kind,
            record_kind,
            scan_root,
            source_reports,
            records,
            warnings,
        );
    }
}

fn should_skip_agent_file(path: &Path) -> bool {
    matches!(file_name_lower(path).as_str(), ".ds_store")
}

fn looks_like_agent_index(path: &Path) -> bool {
    let name = file_name_lower(path);
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let supported_extension = matches!(ext.as_str(), "jsonl" | "json" | "txt" | "md");
    supported_extension
        && (name.contains("index")
            || name.contains("history")
            || name.contains("memor")
            || name.contains("workspace")
            || name.contains("session"))
}

fn scan_supported_file(
    path: &Path,
    source_kind: &str,
    record_kind: RecordKind,
    scan_root: &Path,
    source_reports: &mut Vec<AgentSourceReport>,
    records: &mut Vec<AgentRecord>,
    warnings: &mut Vec<BootstrapWarning>,
) {
    if !path.exists() {
        return;
    }

    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let scanned = match ext.as_str() {
        "jsonl" => scan_jsonl_file(path, source_kind, record_kind, scan_root),
        "json" => scan_json_file(path, source_kind, record_kind, scan_root),
        "txt" | "md" | "" => scan_text_file(path, source_kind, record_kind, scan_root),
        _ => None,
    };

    let Some((report, mut file_records)) = scanned else {
        return;
    };

    warnings.extend(report.warnings.iter().cloned());
    source_reports.push(report);
    records.append(&mut file_records);
}

fn scan_jsonl_file(
    path: &Path,
    source_kind: &str,
    record_kind: RecordKind,
    scan_root: &Path,
) -> Option<(AgentSourceReport, Vec<AgentRecord>)> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) => {
            let warning = BootstrapWarning::new(
                "source_unreadable",
                format!("Unable to read source file: {error}"),
                Some(path),
            );
            return Some((
                source_report(source_kind, path, 0, 0, vec![warning]),
                Vec::new(),
            ));
        }
    };

    let mut records = Vec::new();
    let mut source_warnings = Vec::new();
    let mut records_seen = 0;
    let mut records_parsed = 0;
    for (index, line) in BufReader::new(file).lines().enumerate() {
        let line_number = index + 1;
        let Ok(line) = line else {
            source_warnings.push(BootstrapWarning::new(
                "source_line_unreadable",
                format!("Unable to read line {line_number}."),
                Some(path),
            ));
            continue;
        };
        if line.trim().is_empty() {
            continue;
        }
        records_seen += 1;
        match serde_json::from_str::<Value>(&line) {
            Ok(value) => {
                records_parsed += 1;
                if let Some(record) =
                    record_from_json(value, path, source_kind, record_kind, scan_root)
                {
                    records.push(record);
                }
            }
            Err(error) => source_warnings.push(BootstrapWarning::new(
                "invalid_jsonl_record",
                format!("Invalid JSONL record on line {line_number}: {error}"),
                Some(path),
            )),
        }
    }

    Some((
        source_report(
            source_kind,
            path,
            records_seen,
            records_parsed,
            source_warnings,
        ),
        records,
    ))
}

fn scan_json_file(
    path: &Path,
    source_kind: &str,
    record_kind: RecordKind,
    scan_root: &Path,
) -> Option<(AgentSourceReport, Vec<AgentRecord>)> {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) => {
            let warning = BootstrapWarning::new(
                "source_unreadable",
                format!("Unable to read source file: {error}"),
                Some(path),
            );
            return Some((
                source_report(source_kind, path, 0, 0, vec![warning]),
                Vec::new(),
            ));
        }
    };

    let value = match serde_json::from_str::<Value>(&raw) {
        Ok(value) => value,
        Err(error) => {
            let warning = BootstrapWarning::new(
                "invalid_json_source",
                format!("Unable to parse JSON source: {error}"),
                Some(path),
            );
            return Some((
                source_report(source_kind, path, 1, 0, vec![warning]),
                Vec::new(),
            ));
        }
    };

    let values = match value {
        Value::Array(values) => values,
        value => vec![value],
    };
    let records_seen = values.len();
    let mut records = Vec::new();
    for value in values {
        if let Some(record) = record_from_json(value, path, source_kind, record_kind, scan_root) {
            records.push(record);
        }
    }
    Some((
        source_report(source_kind, path, records_seen, records_seen, Vec::new()),
        records,
    ))
}

fn scan_text_file(
    path: &Path,
    source_kind: &str,
    record_kind: RecordKind,
    scan_root: &Path,
) -> Option<(AgentSourceReport, Vec<AgentRecord>)> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            let warning = BootstrapWarning::new(
                "source_unreadable",
                format!("Unable to read source file: {error}"),
                Some(path),
            );
            return Some((
                source_report(source_kind, path, 0, 0, vec![warning]),
                Vec::new(),
            ));
        }
    };

    let mut records_seen = 0;
    let mut safe_text_fragments = Vec::new();
    let mut cwd_paths = Vec::new();
    let raw = String::from_utf8_lossy(&bytes);
    for line in raw.lines() {
        if line.trim().is_empty() {
            continue;
        }
        records_seen += 1;
        cwd_paths.extend(extract_path_tokens(line, scan_root, true));
        safe_text_fragments.push(line.to_string());
    }

    let records = if cwd_paths.is_empty() && safe_text_fragments.is_empty() {
        Vec::new()
    } else {
        vec![AgentRecord {
            source_ref: source_ref(source_kind, path),
            record_kind,
            cwd_paths,
            safe_text_fragments,
            timestamp: None,
        }]
    };

    Some((
        source_report(source_kind, path, records_seen, records_seen, Vec::new()),
        records,
    ))
}

fn record_from_json(
    value: Value,
    source_path: &Path,
    source_kind: &str,
    record_kind: RecordKind,
    scan_root: &Path,
) -> Option<AgentRecord> {
    let mut collector = JsonRecordCollector::default();
    collect_json_record(&value, None, scan_root, &mut collector);
    if collector.cwd_paths.is_empty() && collector.safe_text_fragments.is_empty() {
        return None;
    }

    Some(AgentRecord {
        source_ref: source_ref(source_kind, source_path),
        record_kind,
        cwd_paths: collector.cwd_paths,
        safe_text_fragments: collector.safe_text_fragments,
        timestamp: collector.timestamp,
    })
}

fn collect_json_record(
    value: &Value,
    key: Option<&str>,
    scan_root: &Path,
    collector: &mut JsonRecordCollector,
) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                let normalized = normalize_key(key);
                if DANGEROUS_JSON_KEYS.contains(&normalized.as_str()) {
                    continue;
                }
                collect_json_record(value, Some(&normalized), scan_root, collector);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_json_record(value, key, scan_root, collector);
            }
        }
        Value::String(text) => {
            if let Some(key) = key {
                if PATH_KEYS.contains(&key) {
                    collector
                        .cwd_paths
                        .extend(extract_path_tokens(text, scan_root, false));
                }
                if SAFE_TEXT_KEYS.contains(&key) {
                    collector.safe_text_fragments.push(text.clone());
                }
                if TIMESTAMP_KEYS.contains(&key) {
                    collector.timestamp = Some(text.clone());
                }
            }
        }
        Value::Number(number) => {
            if key.is_some_and(|key| TIMESTAMP_KEYS.contains(&key)) {
                collector.timestamp = Some(number.to_string());
            }
        }
        Value::Bool(_) | Value::Null => {}
    }
}

fn source_report(
    source_kind: &str,
    path: &Path,
    records_seen: usize,
    records_parsed: usize,
    warnings: Vec<BootstrapWarning>,
) -> AgentSourceReport {
    AgentSourceReport {
        kind: source_kind.to_string(),
        path: path.to_string_lossy().to_string(),
        status: if warnings.is_empty() {
            "scanned".to_string()
        } else {
            "warning".to_string()
        },
        records_seen,
        records_parsed,
        warnings,
    }
}

fn build_projects(
    records: Vec<AgentRecord>,
    warnings: &mut Vec<BootstrapWarning>,
) -> BTreeMap<String, ProjectBuilder> {
    let mut projects = BTreeMap::<String, ProjectBuilder>::new();
    for record in records {
        let paths = unique_paths(record.cwd_paths);
        if paths.is_empty() {
            continue;
        }
        for path in paths {
            let Some(project_path) = valid_project_path(&path, warnings) else {
                continue;
            };
            let key = project_path.to_string_lossy().to_string();
            let project = projects.entry(key).or_insert_with(|| ProjectBuilder {
                path: project_path.clone(),
                ..ProjectBuilder::default()
            });
            project.sources.insert(record.source_ref.clone());
            match record.record_kind {
                RecordKind::Session => project.session_count += 1,
                RecordKind::Memory => project.memory_count += 1,
            }
            update_seen_range(project, record.timestamp.as_deref());
            for text in &record.safe_text_fragments {
                project.safe_texts.push(SourceText {
                    source_ref: record.source_ref.clone(),
                    text: text.clone(),
                    timestamp: record.timestamp.clone(),
                });
            }
        }
    }

    projects
}

fn valid_project_path(path: &Path, warnings: &mut Vec<BootstrapWarning>) -> Option<PathBuf> {
    if !path.exists() {
        warnings.push(BootstrapWarning::new(
            "project_path_missing",
            "Discovered project path does not exist.",
            Some(path),
        ));
        return None;
    }
    if !path.is_dir() {
        warnings.push(BootstrapWarning::new(
            "project_path_not_directory",
            "Discovered project path is not a directory.",
            Some(path),
        ));
        return None;
    }
    match path.canonicalize() {
        Ok(path) => Some(path),
        Err(error) => {
            warnings.push(BootstrapWarning::new(
                "project_path_unresolved",
                format!("Unable to resolve discovered project path: {error}"),
                Some(path),
            ));
            None
        }
    }
}

fn scan_project_manifests(
    projects: &mut BTreeMap<String, ProjectBuilder>,
    warnings: &mut Vec<BootstrapWarning>,
) {
    for project in projects.values_mut() {
        for file_name in MANIFEST_FILES {
            let manifest_path = project.path.join(file_name);
            if !manifest_path.exists() || !manifest_path.is_file() {
                continue;
            }
            match read_manifest_evidence(&manifest_path, file_name) {
                Ok(evidence) => project.manifests.push(evidence),
                Err(error) => warnings.push(BootstrapWarning::new(
                    "manifest_unreadable",
                    format!("Unable to read manifest: {error}"),
                    Some(&manifest_path),
                )),
            }
        }
    }
}

fn filter_non_project_paths(projects: &mut BTreeMap<String, ProjectBuilder>, scan_root: &Path) {
    projects.retain(|_, project| {
        !(project.manifests.is_empty() && is_non_project_path(&project.path, scan_root))
    });
}

fn is_non_project_path(path: &Path, scan_root: &Path) -> bool {
    if path == Path::new("/") || path == scan_root {
        return true;
    }

    if path.starts_with(scan_root)
        && path
            .strip_prefix(scan_root)
            .ok()
            .and_then(|relative| relative.components().next())
            .and_then(|component| component.as_os_str().to_str())
            .is_some_and(|first| first.starts_with('.'))
    {
        return true;
    }

    let path_text = path.to_string_lossy();
    path_text.starts_with("/opt/homebrew/Cellar/")
        || path_text.starts_with("/usr/local/Cellar/")
        || path_text.contains("/.nvm/")
        || path_text.contains("/Library/Application Support/")
}

fn read_manifest_evidence(path: &Path, file_name: &str) -> Result<ManifestEvidence> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("unable to read manifest {}", path.display()))?;
    let mut tech_terms = BTreeSet::new();
    let mut keyword_terms = BTreeSet::new();

    add_manifest_baseline(file_name, &mut tech_terms, &mut keyword_terms);
    for term in recognized_terms(&raw) {
        if let Some(tech) = canonical_tech_term(&term) {
            tech_terms.insert(tech);
        } else if let Some(keyword) = canonical_keyword_term(&term) {
            keyword_terms.insert(keyword);
        }
    }

    extract_manifest_specific_terms(file_name, &raw, &mut tech_terms, &mut keyword_terms);

    Ok(ManifestEvidence {
        path: path.to_path_buf(),
        file_name: file_name.to_string(),
        tech_terms: tech_terms.into_iter().collect(),
        keyword_terms: keyword_terms.into_iter().collect(),
    })
}

fn add_manifest_baseline(
    file_name: &str,
    tech_terms: &mut BTreeSet<String>,
    keyword_terms: &mut BTreeSet<String>,
) {
    match file_name {
        "Cargo.toml" => {
            tech_terms.insert("Rust".to_string());
            keyword_terms.insert("cargo".to_string());
        }
        "package.json" => {
            tech_terms.insert("JavaScript".to_string());
            tech_terms.insert("Node.js".to_string());
        }
        "go.mod" => {
            tech_terms.insert("Go".to_string());
        }
        "pyproject.toml" | "requirements.txt" => {
            tech_terms.insert("Python".to_string());
        }
        "pom.xml" => {
            tech_terms.insert("Java".to_string());
            keyword_terms.insert("maven".to_string());
        }
        "build.gradle" | "settings.gradle" => {
            tech_terms.insert("Java".to_string());
            keyword_terms.insert("gradle".to_string());
        }
        "Gemfile" => {
            tech_terms.insert("Ruby".to_string());
        }
        "composer.json" => {
            tech_terms.insert("PHP".to_string());
        }
        _ => {}
    }
}

fn extract_manifest_specific_terms(
    file_name: &str,
    raw: &str,
    tech_terms: &mut BTreeSet<String>,
    keyword_terms: &mut BTreeSet<String>,
) {
    if file_name == "package.json" {
        if raw.contains("\"typescript\"") || raw.contains("\"ts-node\"") {
            tech_terms.insert("TypeScript".to_string());
        }
        if raw.contains("\"react\"") || raw.contains("\"@types/react\"") {
            tech_terms.insert("React".to_string());
            keyword_terms.insert("frontend".to_string());
            keyword_terms.insert("component".to_string());
        }
        if raw.contains("\"vue\"") {
            tech_terms.insert("Vue".to_string());
            keyword_terms.insert("frontend".to_string());
        }
        if raw.contains("\"svelte\"") {
            tech_terms.insert("Svelte".to_string());
            keyword_terms.insert("frontend".to_string());
        }
    }
    if matches!(file_name, "pyproject.toml" | "requirements.txt") {
        if raw.contains("pandas") {
            keyword_terms.insert("data".to_string());
            keyword_terms.insert("pandas".to_string());
        }
        if raw.contains("pytest") {
            keyword_terms.insert("testing".to_string());
            keyword_terms.insert("pytest".to_string());
        }
    }
}

fn build_outputs(
    projects: BTreeMap<String, ProjectBuilder>,
) -> (
    Vec<ActiveProject>,
    Vec<EvidenceOutput>,
    Vec<EvidenceOutput>,
    Vec<RecentTaskTheme>,
) {
    let mut active_projects = Vec::new();
    let mut tech_evidence = BTreeMap::<String, EvidenceBuilder>::new();
    let mut keyword_evidence = BTreeMap::<String, EvidenceBuilder>::new();
    let mut themes = BTreeMap::<String, ThemeBuilder>::new();

    let mut projects = projects.into_values().collect::<Vec<_>>();
    projects.sort_by(|left, right| {
        right
            .manifests
            .len()
            .cmp(&left.manifests.len())
            .then_with(|| right.session_count.cmp(&left.session_count))
            .then_with(|| right.memory_count.cmp(&left.memory_count))
            .then_with(|| left.path.cmp(&right.path))
    });

    for (index, project) in projects.into_iter().enumerate() {
        let project_ref = format!("project:{}", index + 1);
        let path = project.path.to_string_lossy().to_string();

        add_project_path_evidence(
            &project.path,
            &project_ref,
            &mut tech_evidence,
            &mut keyword_evidence,
        );
        for source_text in &project.safe_texts {
            add_text_evidence(
                source_text,
                &project_ref,
                &mut tech_evidence,
                &mut keyword_evidence,
                &mut themes,
            );
        }

        let mut manifest_paths = Vec::new();
        for manifest in &project.manifests {
            let manifest_ref = format!("{project_ref}:{}", manifest.file_name);
            manifest_paths.push(manifest.path.to_string_lossy().to_string());
            for term in &manifest.tech_terms {
                add_term_evidence(
                    &mut tech_evidence,
                    term,
                    12,
                    format!("manifest:{}", manifest.file_name),
                    Some(project_ref.clone()),
                    Some(manifest_ref.clone()),
                    None,
                );
            }
            for term in &manifest.keyword_terms {
                add_term_evidence(
                    &mut keyword_evidence,
                    term,
                    8,
                    format!("manifest:{}", manifest.file_name),
                    Some(project_ref.clone()),
                    Some(manifest_ref.clone()),
                    Some("manifest dependency or ecosystem signal".to_string()),
                );
            }
        }

        let mut sources = project.sources.into_iter().collect::<Vec<_>>();
        sources.sort();
        manifest_paths.sort();
        active_projects.push(ActiveProject {
            id: project_ref,
            path,
            first_seen_at: project.first_seen_at,
            last_seen_at: project.last_seen_at,
            session_count: project.session_count,
            memory_count: project.memory_count,
            manifest_count: manifest_paths.len(),
            sources,
            manifests: manifest_paths,
        });
    }

    (
        active_projects,
        evidence_outputs(tech_evidence),
        evidence_outputs(keyword_evidence),
        theme_outputs(themes),
    )
}

fn add_project_path_evidence(
    path: &Path,
    project_ref: &str,
    tech_evidence: &mut BTreeMap<String, EvidenceBuilder>,
    keyword_evidence: &mut BTreeMap<String, EvidenceBuilder>,
) {
    let text = path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join(" ");
    for term in recognized_terms(&text) {
        if let Some(tech) = canonical_tech_term(&term) {
            add_term_evidence(
                tech_evidence,
                &tech,
                2,
                "project:path".to_string(),
                Some(project_ref.to_string()),
                None,
                None,
            );
        } else if let Some(keyword) = canonical_keyword_term(&term) {
            add_term_evidence(
                keyword_evidence,
                &keyword,
                2,
                "project:path".to_string(),
                Some(project_ref.to_string()),
                None,
                Some("project path signal".to_string()),
            );
        }
    }
}

fn add_text_evidence(
    source_text: &SourceText,
    project_ref: &str,
    tech_evidence: &mut BTreeMap<String, EvidenceBuilder>,
    keyword_evidence: &mut BTreeMap<String, EvidenceBuilder>,
    themes: &mut BTreeMap<String, ThemeBuilder>,
) {
    for term in recognized_terms(&source_text.text) {
        if let Some(tech) = canonical_tech_term(&term) {
            add_term_evidence(
                tech_evidence,
                &tech,
                3,
                source_text.source_ref.clone(),
                Some(project_ref.to_string()),
                None,
                None,
            );
        } else if let Some(keyword) = canonical_keyword_term(&term) {
            add_term_evidence(
                keyword_evidence,
                &keyword,
                3,
                source_text.source_ref.clone(),
                Some(project_ref.to_string()),
                None,
                Some("Agent title, summary, or memory signal".to_string()),
            );
            let theme = themes.entry(keyword).or_default();
            theme.count += 1;
            theme.sources.insert(source_text.source_ref.clone());
            update_optional_last_seen(&mut theme.last_seen_at, source_text.timestamp.as_deref());
        }
    }
}

fn add_term_evidence(
    evidence: &mut BTreeMap<String, EvidenceBuilder>,
    term: &str,
    weight: i32,
    source: String,
    project_ref: Option<String>,
    manifest_ref: Option<String>,
    reason: Option<String>,
) {
    let item = evidence.entry(term.to_string()).or_default();
    item.weight += weight;
    item.count += 1;
    item.sources.insert(source);
    if let Some(project_ref) = project_ref {
        item.project_refs.insert(project_ref);
    }
    if let Some(manifest_ref) = manifest_ref {
        item.manifest_refs.insert(manifest_ref);
    }
    if let Some(reason) = reason {
        item.reasons.insert(reason);
    }
}

fn evidence_outputs(evidence: BTreeMap<String, EvidenceBuilder>) -> Vec<EvidenceOutput> {
    let mut output = evidence
        .into_iter()
        .map(|(term, builder)| EvidenceOutput {
            term,
            weight: builder.weight,
            count: builder.count,
            sources: builder.sources.into_iter().collect(),
            project_refs: builder.project_refs.into_iter().collect(),
            manifest_refs: builder.manifest_refs.into_iter().collect(),
            reason: if builder.reasons.is_empty() {
                None
            } else {
                Some(builder.reasons.into_iter().collect::<Vec<_>>().join("; "))
            },
        })
        .collect::<Vec<_>>();
    output.sort_by(|left, right| {
        right
            .weight
            .cmp(&left.weight)
            .then_with(|| right.count.cmp(&left.count))
            .then_with(|| left.term.cmp(&right.term))
    });
    output
}

fn theme_outputs(themes: BTreeMap<String, ThemeBuilder>) -> Vec<RecentTaskTheme> {
    let mut output = themes
        .into_iter()
        .map(|(theme, builder)| RecentTaskTheme {
            theme,
            count: builder.count,
            sources: builder.sources.into_iter().collect(),
            last_seen_at: builder.last_seen_at,
        })
        .collect::<Vec<_>>();
    output.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| right.last_seen_at.cmp(&left.last_seen_at))
            .then_with(|| left.theme.cmp(&right.theme))
    });
    output
}

fn recommended_profile(
    tech_stack_evidence: &[EvidenceOutput],
    keyword_evidence: &[EvidenceOutput],
    recent_task_themes: &[RecentTaskTheme],
) -> RecommendedProfile {
    let tech_stack = tech_stack_evidence
        .iter()
        .take(12)
        .map(|item| item.term.clone())
        .collect::<Vec<_>>();
    let tech_terms = tech_stack
        .iter()
        .map(|term| term.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    let mut keywords = Vec::new();
    for item in keyword_evidence {
        if keywords.len() >= 20 {
            break;
        }
        if tech_terms.contains(&item.term.to_ascii_lowercase()) {
            continue;
        }
        keywords.push(item.term.clone());
    }
    for theme in recent_task_themes {
        if keywords.len() >= 20 {
            break;
        }
        if !keywords.contains(&theme.theme) && !tech_terms.contains(&theme.theme) {
            keywords.push(theme.theme.clone());
        }
    }
    RecommendedProfile {
        tech_stack,
        keywords,
    }
}

fn recognized_terms(text: &str) -> Vec<String> {
    text.split(|character: char| !character.is_ascii_alphanumeric() && character != '-')
        .map(|token| token.trim().to_ascii_lowercase())
        .filter(|token| token.len() >= 2)
        .collect()
}

fn canonical_tech_term(term: &str) -> Option<String> {
    match term {
        "ai" => Some("AI".to_string()),
        "astro" => Some("Astro".to_string()),
        "go" | "golang" => Some("Go".to_string()),
        "java" => Some("Java".to_string()),
        "javascript" | "js" => Some("JavaScript".to_string()),
        "kotlin" => Some("Kotlin".to_string()),
        "node" | "nodejs" => Some("Node.js".to_string()),
        "php" => Some("PHP".to_string()),
        "python" | "py" => Some("Python".to_string()),
        "react" => Some("React".to_string()),
        "ruby" => Some("Ruby".to_string()),
        "rust" | "cargo" => Some("Rust".to_string()),
        "svelte" => Some("Svelte".to_string()),
        "typescript" | "ts" => Some("TypeScript".to_string()),
        "vue" => Some("Vue".to_string()),
        "yaml" => Some("YAML".to_string()),
        _ => None,
    }
}

fn canonical_keyword_term(term: &str) -> Option<String> {
    match term {
        "agent" | "agents" => Some("agent".to_string()),
        "api" => Some("api".to_string()),
        "axum" => Some("axum".to_string()),
        "backend" => Some("backend".to_string()),
        "browser" => Some("browser".to_string()),
        "clap" | "cli" | "cobra" | "commander" => Some("cli".to_string()),
        "cloud" => Some("cloud".to_string()),
        "component" | "components" => Some("component".to_string()),
        "data" | "pandas" => Some("data".to_string()),
        "developer-tools" | "devtools" | "tooling" => Some("developer-tools".to_string()),
        "docker" => Some("docker".to_string()),
        "eslint" => Some("eslint".to_string()),
        "eval" | "evaluation" => Some("evaluation".to_string()),
        "fastapi" => Some("fastapi".to_string()),
        "form" | "forms" => Some("form".to_string()),
        "frontend" | "ui" => Some("frontend".to_string()),
        "gitops" => Some("gitops".to_string()),
        "graphql" => Some("graphql".to_string()),
        "grpc" => Some("grpc".to_string()),
        "helm" => Some("helm".to_string()),
        "infra" | "infrastructure" => Some("infrastructure".to_string()),
        "jest" => Some("testing".to_string()),
        "k8s" | "kubernetes" => Some("kubernetes".to_string()),
        "llm" | "llms" => Some("llm".to_string()),
        "mcp" => Some("mcp".to_string()),
        "next" | "nextjs" => Some("nextjs".to_string()),
        "openai" => Some("openai".to_string()),
        "operator" | "operators" => Some("operator".to_string()),
        "playwright" => Some("playwright".to_string()),
        "pytest" | "test" | "testing" | "tests" => Some("testing".to_string()),
        "reqwest" => Some("reqwest".to_string()),
        "serde" => Some("serde".to_string()),
        "tailwind" => Some("tailwind".to_string()),
        "tauri" => Some("tauri".to_string()),
        "terraform" => Some("terraform".to_string()),
        "tokio" => Some("tokio".to_string()),
        "vite" | "vitest" => Some("vite".to_string()),
        "wasm" => Some("wasm".to_string()),
        _ => None,
    }
}

fn extract_path_tokens(
    text: &str,
    scan_root: &Path,
    require_existing_directory: bool,
) -> Vec<PathBuf> {
    let mut paths = BTreeSet::new();
    insert_path_token(
        text.trim(),
        scan_root,
        require_existing_directory,
        &mut paths,
    );
    for token in text.split(|character: char| {
        character.is_whitespace()
            || matches!(
                character,
                '"' | '\'' | '`' | ',' | ';' | ')' | '(' | '[' | ']' | '{' | '}'
            )
    }) {
        let cleaned = token
            .trim_matches(|character: char| matches!(character, ':' | '.' | ',' | ';'))
            .trim();
        insert_path_token(cleaned, scan_root, require_existing_directory, &mut paths);
    }
    paths.into_iter().collect()
}

fn insert_path_token(
    token: &str,
    scan_root: &Path,
    require_existing_directory: bool,
    paths: &mut BTreeSet<PathBuf>,
) {
    if token.is_empty() || token.starts_with("http://") || token.starts_with("https://") {
        return;
    }
    let path = if let Some(rest) = token.strip_prefix("~/") {
        scan_root.join(rest)
    } else if token.starts_with('/') {
        PathBuf::from(token)
    } else {
        return;
    };
    if !require_existing_directory || (path.exists() && path.is_dir()) {
        paths.insert(path);
    }
}

fn unique_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn update_seen_range(project: &mut ProjectBuilder, timestamp: Option<&str>) {
    let Some(timestamp) = timestamp.filter(|timestamp| !timestamp.trim().is_empty()) else {
        return;
    };
    let timestamp = timestamp.to_string();
    if project
        .first_seen_at
        .as_ref()
        .is_none_or(|current| timestamp < *current)
    {
        project.first_seen_at = Some(timestamp.clone());
    }
    if project
        .last_seen_at
        .as_ref()
        .is_none_or(|current| timestamp > *current)
    {
        project.last_seen_at = Some(timestamp);
    }
}

fn update_optional_last_seen(target: &mut Option<String>, timestamp: Option<&str>) {
    let Some(timestamp) = timestamp.filter(|timestamp| !timestamp.trim().is_empty()) else {
        return;
    };
    let timestamp = timestamp.to_string();
    if target.as_ref().is_none_or(|current| timestamp > *current) {
        *target = Some(timestamp);
    }
}

fn source_ref(source_kind: &str, path: &Path) -> String {
    format!("{source_kind}:{}", path.to_string_lossy())
}

fn file_name_lower(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn normalize_key(key: &str) -> String {
    let mut normalized = String::new();
    for (index, character) in key.chars().enumerate() {
        if character == '-' || character == ' ' {
            normalized.push('_');
        } else if character.is_ascii_uppercase() {
            if index > 0 {
                normalized.push('_');
            }
            normalized.push(character.to_ascii_lowercase());
        } else {
            normalized.push(character);
        }
    }
    while normalized.contains("__") {
        normalized = normalized.replace("__", "_");
    }
    normalized
}

impl BootstrapWarning {
    fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        path: Option<&Path>,
    ) -> BootstrapWarning {
        BootstrapWarning {
            code: code.into(),
            message: message.into(),
            path: path.map(|path| path.to_string_lossy().to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{canonical_keyword_term, canonical_tech_term, read_manifest_evidence};
    use tempfile::tempdir;

    #[test]
    fn canonicalizes_manifest_terms() {
        assert_eq!(canonical_tech_term("rust").as_deref(), Some("Rust"));
        assert_eq!(
            canonical_tech_term("typescript").as_deref(),
            Some("TypeScript")
        );
        assert_eq!(canonical_keyword_term("pytest").as_deref(), Some("testing"));
        assert_eq!(
            canonical_keyword_term("devtools").as_deref(),
            Some("developer-tools")
        );
    }

    #[test]
    fn package_manifest_extracts_frontend_signals() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("package.json");
        std::fs::write(
            &path,
            r#"{"dependencies":{"react":"latest","vite":"latest","typescript":"latest"}}"#,
        )
        .unwrap();
        let evidence = read_manifest_evidence(&path, "package.json").unwrap();
        assert!(evidence.tech_terms.contains(&"JavaScript".to_string()));
        assert!(evidence.tech_terms.contains(&"TypeScript".to_string()));
        assert!(evidence.tech_terms.contains(&"React".to_string()));
        assert!(evidence.keyword_terms.contains(&"vite".to_string()));
    }
}
