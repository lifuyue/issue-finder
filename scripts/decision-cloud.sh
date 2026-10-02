#!/usr/bin/env bash
# Versioned Cloud profile. Secrets remain runtime environment variables.
set -euo pipefail

configure_only=false
issue_finder_binary='issue-finder'
while [[ $# -gt 0 ]]; do
    case "$1" in
        --configure-only) configure_only=true; shift ;;
        --issue-finder-binary)
            [[ $# -ge 2 && -n "$2" ]] || { echo 'Missing binary path' >&2; exit 1; }
            issue_finder_binary="$2"; shift 2 ;;
        --help|-h)
            echo 'Usage: decision-cloud.sh [--configure-only] [--issue-finder-binary PATH]'
            echo 'Configure Clef, concurrency 8, budget 24, timeout 45s; then make one real check.'
            echo '--configure-only writes non-secret configuration without authentication or network checks.'
            exit 0 ;;
        *) echo 'Unknown option; use --help' >&2; exit 1 ;;
    esac
done

issue_finder_binary="$(command -v -- "$issue_finder_binary")"
"$issue_finder_binary" decision-configure --help >/dev/null
if [[ "$configure_only" == false ]]; then
    # Python reads environment keys containing hyphens without shell indirection.
    python3 - <<'PY'
import os
import re
import sys

token_selector = "ISSUE_FINDER_CLOUDFLARE_API_TOKEN_ENV"
account_selector = "ISSUE_FINDER_CLOUDFLARE_ACCOUNT_ID_ENV"
if (token_selector in os.environ) != (account_selector in os.environ):
    print(f"{token_selector} and {account_selector} must be set together", file=sys.stderr)
    sys.exit(1)
values = {}
for selector, default_key in (
    (token_selector, "CLOUDFLARE_API_TOKEN"),
    (account_selector, "CLOUDFLARE_ACCOUNT_ID"),
):
    key = os.environ.get(selector, default_key)
    if not re.fullmatch(r"[A-Za-z0-9_-]+", key):
        print(f"{selector} must be a nonempty environment variable name containing only ASCII letters, digits, underscores, or hyphens", file=sys.stderr)
        sys.exit(1)
    value = os.environ.get(key, "").strip()
    if not value:
        print(f"{key} is required at task startup", file=sys.stderr)
        sys.exit(1)
    values[selector] = value
if not re.fullmatch(r"[A-Za-z0-9_-]+", values[account_selector]):
    print("Selected Cloudflare account ID must contain only ASCII letters, digits, underscores, or hyphens", file=sys.stderr)
    sys.exit(1)
PY
fi

configure() {
    "$issue_finder_binary" decision-configure \
        --provider cloudflare-clef-flash --concurrency 8 \
        --candidate-budget 24 --timeout-seconds 45
}
if [[ "$configure_only" == true ]]; then
    configure
    echo 'Cloud profile configured; authentication has not been verified.' >&2
else
    configure >&2
    # No override: verify the persisted provider that scout will actually use.
    exec "$issue_finder_binary" decision-check
fi
