//! Explicit Cloud bootstrap. The model provider only consumes an initialized home.
use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};

use crate::paths::IssueFinderPaths;

pub const AUTH_ENV: &str = "ISSUE_FINDER_CODEX_AUTH_JSON";
pub const HOME_ENV: &str = "ISSUE_FINDER_CODEX_HOME";
const MAX_AUTH_BYTES: usize = 128 * 1024;

fn selected_home(paths: &IssueFinderPaths) -> Result<PathBuf> {
    let home = match env::var_os(HOME_ENV) {
        Some(value) => PathBuf::from(value),
        None => paths.decision_codex_home(),
    };
    ensure!(
        home.is_absolute() && !home.components().any(|part| part == Component::ParentDir),
        "ISSUE_FINDER_CODEX_HOME must be an absolute private directory without parent traversal"
    );
    // A separate selector must never overwrite the main agent's credential store.
    let same_path = |other: &Path| {
        home == other
            || fs::canonicalize(&home)
                .ok()
                .zip(fs::canonicalize(other).ok())
                .is_some_and(|(a, b)| a == b)
    };
    ensure!(
        !env::var_os("CODEX_HOME").is_some_and(|value| same_path(Path::new(&value)))
            && !dirs::home_dir().is_some_and(|root| same_path(&root.join(".codex"))),
        "Decision model authentication must use a separate home from the main Codex login"
    );
    Ok(home)
}

/// No secret content is returned, including on parse or filesystem failures.
pub fn initialize(paths: &IssueFinderPaths, replace: bool) -> Result<Value> {
    let raw = env::var(AUTH_ENV).context("ISSUE_FINDER_CODEX_AUTH_JSON is missing or not UTF-8")?;
    let contents = validated_contents(&raw)?;
    let home = selected_home(paths)?;
    let initialized = write_auth(&home, &contents, replace)?;
    Ok(
        json!({"success":true,"status":if initialized {"initialized"} else {"existing_preserved"},
        "codexHome":home,"authenticationVerified":false,
        "nextAction":"Run issue-finder decision-check with the configured Codex CLI; file creation does not verify authentication."}),
    )
}

fn validated_contents(raw: &str) -> Result<Vec<u8>> {
    ensure!(
        !raw.trim().is_empty() && raw.len() <= MAX_AUTH_BYTES,
        "ISSUE_FINDER_CODEX_AUTH_JSON must contain a nonempty auth.json object of at most 128 KiB"
    );
    let value: Value = serde_json::from_str(raw)
        .map_err(|_| anyhow::anyhow!("ISSUE_FINDER_CODEX_AUTH_JSON is not valid JSON; supply the complete file contents, not a proxy placeholder or base64"))?;
    ensure!(value.is_object(), "Codex auth.json must be a JSON object");
    let mode = value.get("auth_mode").filter(|v| !v.is_null());
    ensure!(mode.is_none() || matches!(mode.and_then(Value::as_str), Some("chatgpt" | "apikey")),
        "Only refreshable ChatGPT or API-key file authentication is supported; external chatgptAuthTokens are not a portable login");
    let nonempty = |v: Option<&Value>| {
        v.and_then(Value::as_str)
            .is_some_and(|v| !v.trim().is_empty())
    };
    let key = nonempty(value.get("OPENAI_API_KEY"));
    let has_token_data = value.get("tokens").is_some_and(|v| !v.is_null());
    ensure!(
        value
            .get("OPENAI_API_KEY")
            .is_none_or(|v| v.is_null() || v.is_string()),
        "Codex OPENAI_API_KEY must be a string or null"
    );
    let tokens = ["id_token", "access_token", "refresh_token"]
        .iter()
        .all(|key| nonempty(value.get("tokens").and_then(|v| v.get(key))));
    let valid = match mode.and_then(Value::as_str) {
        Some("chatgpt") => tokens && !key,
        Some("apikey") => key && !has_token_data,
        None => (key && !has_token_data) || (tokens && !key),
        _ => false,
    };
    ensure!(valid, "Codex auth.json must contain one coherent credential type: ChatGPT id/access/refresh tokens or OPENAI_API_KEY");
    // Preserve the original Codex-managed fields; this checks shape, not validity or model access.
    serde_json::to_vec(&value).context("Cannot encode Codex authentication")
}

fn private_directory(home: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(home).context("Cannot inspect the isolated Codex home")?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "Isolated Codex home must be a real directory, not a symlink"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            metadata.uid() == unsafe { libc::geteuid() } && metadata.mode() & 0o777 == 0o700,
            "Isolated Codex home must be owned by the current user with mode 0700"
        );
    }
    Ok(())
}

fn auth_exists(home: &Path) -> Result<bool> {
    match fs::symlink_metadata(home.join("auth.json")) {
        Ok(metadata) => {
            ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "Isolated auth.json must be a regular file, not a symlink"
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                ensure!(
                    metadata.uid() == unsafe { libc::geteuid() }
                        && metadata.mode() & 0o777 == 0o600
                        && metadata.nlink() == 1,
                    "Isolated auth.json must be privately owned with mode 0600 and no hard links"
                );
            }
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => bail!("Cannot inspect isolated auth.json"),
    }
}

#[cfg(unix)]
fn write_auth(home: &Path, contents: &[u8], replace: bool) -> Result<bool> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(home)
        .context("Cannot create the isolated Codex home")?;
    private_directory(home)?;
    // Serialize bootstrap writers, including the brief two-link publication window.
    // The lock is released by the OS on errors or process exit; it contains no credentials.
    let lock = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(home.join(".auth-init.lock"))
        .context("Cannot open authentication initialization lock")?;
    lock.lock()
        .context("Cannot lock authentication initialization")?;
    if auth_exists(home)? && !replace {
        return Ok(false);
    }
    let temporary = home.join(format!(
        ".auth-{}-{}.tmp",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temporary)
        .context("Cannot create private authentication staging file")?;
    let outcome = (|| {
        file.write_all(contents)
            .context("Cannot write authentication staging file")?;
        file.sync_all()
            .context("Cannot persist authentication staging file")?;
        if replace {
            fs::rename(&temporary, home.join("auth.json"))
                .context("Cannot replace isolated auth.json")?;
            Ok(true)
        } else {
            // Atomic no-clobber publication: other starts may already have seeded or refreshed it.
            match fs::hard_link(&temporary, home.join("auth.json")) {
                Ok(()) => Ok(true),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
                Err(_) => bail!("Cannot publish isolated auth.json"),
            }
        }
    })();
    // Removing our staging link does not remove the published credential.
    match fs::remove_file(&temporary) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && replace => {}
        Err(_) => bail!("Cannot remove authentication staging file"),
    }
    let initialized = outcome?;
    ensure!(
        auth_exists(home)?,
        "Isolated auth.json is missing after initialization"
    );
    Ok(initialized)
}

#[cfg(not(unix))]
fn write_auth(_home: &Path, _contents: &[u8], _replace: bool) -> Result<bool> {
    bail!("Cloud auth.json initialization currently requires Unix permission support; provision Codex authentication through the host")
}

/// Resolve an opt-in runtime home without reading or rewriting authentication.
pub(super) fn runtime_home() -> Result<Option<PathBuf>> {
    if env::var_os(HOME_ENV).is_none() && env::var_os(AUTH_ENV).is_none() {
        return Ok(None);
    }
    let home = selected_home(&IssueFinderPaths::resolve()?)?;
    private_directory(&home)
        .context("Run issue-finder decision-auth-init before using injected authentication")?;
    ensure!(
        auth_exists(&home)?,
        "Isolated auth.json is absent; run issue-finder decision-auth-init before decision model calls"
    );
    Ok(Some(home))
}

pub(super) fn configure_command(command: &mut std::process::Command, home: Option<&Path>) {
    command.env_remove(AUTH_ENV);
    if let Some(home) = home {
        command
            .env("CODEX_HOME", home)
            .env_remove("CODEX_ACCESS_TOKEN")
            .env_remove("CODEX_API_KEY")
            .env_remove("OPENAI_API_KEY");
    }
}
