#!/usr/bin/env bash
# Install only with an explicit Cloud mode; always verify with the production adapter.
set -euo pipefail

readonly tested_version='0.159.3'
mode='check'
auth_from_env=false
prefix="${ISSUE_FINDER_HOME:-${HOME}/.issue-finder}/system1/codex-cli"
codex_binary="${ISSUE_FINDER_CODEX_BIN:-}"
issue_finder_binary='issue-finder'

usage() {
    cat <<'USAGE'
Usage: decision-codex.sh [--check-only | --cloud-install] [options]
  --check-only                 Reuse existing Codex CLI and login (default).
  --cloud-install              Install Codex CLI 0.159.3 in a private npm prefix.
  --auth-from-env              Seed isolated auth from ISSUE_FINDER_CODEX_AUTH_JSON.
  --prefix PATH                Cloud npm prefix; does not modify global npm/PATH.
  --codex-binary PATH           Existing CLI to check (check-only mode).
  --issue-finder-binary PATH    Issue Finder binary exposing decision-check.
  --help                       Show this help.

A successful check makes a real gpt-6-luna / reasoning=none structured request.
It requires working Codex authentication and network access. --auth-from-env
explicitly runs decision-auth-init and preserves any existing refreshed auth file.
Credential values must be injected through the environment, never arguments.
Diagnostic stdout is one JSON object; redacted setup logs use stderr.
USAGE
}
fail() { printf '%s\n' "$*" >&2; exit 1; }
need_value() { [[ $# -ge 2 && -n "$2" ]] || fail "Missing value for $1"; }
while [[ $# -gt 0 ]]; do
    case "$1" in
        --check-only) mode='check'; shift ;;
        --cloud-install) mode='cloud'; shift ;;
        --auth-from-env) auth_from_env=true; shift ;;
        --prefix) need_value "$@"; prefix="$2"; shift 2 ;;
        --codex-binary) need_value "$@"; codex_binary="$2"; shift 2 ;;
        --issue-finder-binary) need_value "$@"; issue_finder_binary="$2"; shift 2 ;;
        --help|-h) usage; exit 0 ;;
        *) fail "Unknown option: $1" ;;
    esac
done

# Locate the production diagnostic before doing any installation.
issue_finder_binary="$(command -v -- "$issue_finder_binary")" || fail 'Issue Finder not found; install a release with decision-check first.'
[[ -x "$issue_finder_binary" ]] || fail 'Issue Finder binary is not executable.'
env -u ISSUE_FINDER_CODEX_AUTH_JSON "$issue_finder_binary" decision-check --help >/dev/null || fail 'Installed Issue Finder lacks the Decision model diagnostic.'
if [[ "$auth_from_env" == true ]]; then
    env -u ISSUE_FINDER_CODEX_AUTH_JSON "$issue_finder_binary" decision-auth-init --help >/dev/null || fail 'Installed Issue Finder lacks decision-auth-init; install the supporting source revision first.'
fi

if [[ "$mode" == 'cloud' ]]; then
    [[ -z "$codex_binary" ]] || fail 'Do not combine --cloud-install with --codex-binary or ISSUE_FINDER_CODEX_BIN; unset the override for installation.'
    command -v npm >/dev/null || fail 'Cloud installation requires Node.js and npm in this environment.'
    [[ "$prefix" == /* ]] || fail '--prefix must be an absolute private installation path.'
    printf 'Installing @openai/codex@%s in %s\n' "$tested_version" "$prefix" >&2
    env -u ISSUE_FINDER_CODEX_AUTH_JSON npm install --prefix "$prefix" --no-audit --no-fund --loglevel=error "@openai/codex@${tested_version}" >&2
    codex_binary="${prefix}/node_modules/.bin/codex"
else
    if [[ -z "$codex_binary" ]]; then
        codex_binary="$(command -v codex)" || fail 'Codex CLI not found. Local mode never installs or upgrades it; provide --codex-binary or use an explicitly configured Cloud installation.'
    else
        codex_binary="$(command -v -- "$codex_binary")" || fail 'Configured Codex CLI not found.'
    fi
fi
[[ -x "$codex_binary" ]] || fail 'Selected Codex CLI is not executable.'
if [[ "$auth_from_env" == true ]]; then
    "$issue_finder_binary" decision-auth-init >&2
fi
version="$(env -u ISSUE_FINDER_CODEX_AUTH_JSON "$codex_binary" --version)"
if [[ "$mode" == 'cloud' && "$version" != "codex-cli ${tested_version}" ]]; then
    fail "Installed CLI version mismatch: expected codex-cli ${tested_version}, received ${version}."
fi
printf 'Checking %s (%s); compatibility baseline is %s.\n' "$codex_binary" "$version" "$tested_version" >&2
# Independent context and model/schema/auth verification are owned by the adapter.
exec "$issue_finder_binary" decision-check --provider codex --codex-binary "$codex_binary"
