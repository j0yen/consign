#!/usr/bin/env bash
# consign-cron.sh — wrapper for consign drain, called by consign-drain.service
# Appends a one-line summary to the daily journal ONLY when repos were pushed
# or errors occurred. Silent on a clean (zero-debt) pass.
# Never force-pushes; exits non-zero only on actual push error.

set -euo pipefail

JOURNAL_DIR="${HOME}/brain/journal"
TODAY="$(date +%Y-%m-%d)"
JOURNAL="${JOURNAL_DIR}/${TODAY}.md"

# Check that consign drain subcommand exists; if not, exit 0 (deferred)
if ! consign drain --help &>/dev/null 2>&1; then
    echo "[consign-cron] consign drain not available yet — skipping (deferred)" >&2
    exit 0
fi

RECEIPT_FILE="$(mktemp /tmp/consign-cron.XXXXXX.json)"
trap 'rm -f "$RECEIPT_FILE"' EXIT

# Run drain, capture output and exit code
DRAIN_EXIT=0
consign drain --no-dry-run --format json >"$RECEIPT_FILE" 2>&1 || DRAIN_EXIT=$?

# Parse summary counts from JSON receipt
PUSHED=0
ERRORS=0
NEEDS_HUMAN=0

if command -v jq &>/dev/null && [[ -s "$RECEIPT_FILE" ]]; then
    # Attempt to parse if the output is valid JSON
    if jq -e . "$RECEIPT_FILE" &>/dev/null; then
        PUSHED=$(jq '[.[] | select(.result == "pushed")] | length' "$RECEIPT_FILE" 2>/dev/null || echo 0)
        ERRORS=$(jq '[.[] | select(.result == "error")] | length' "$RECEIPT_FILE" 2>/dev/null || echo 0)
        NEEDS_HUMAN=$(jq '[.[] | select(.result == "needs-human")] | length' "$RECEIPT_FILE" 2>/dev/null || echo 0)
    fi
fi

# Append to journal only when there's something to report
if [[ "$PUSHED" -gt 0 || "$ERRORS" -gt 0 || "$DRAIN_EXIT" -ne 0 ]]; then
    ISO="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    mkdir -p "$JOURNAL_DIR"
    printf '%s  consign-cron: pushed %d repos, %d errors, %d needs-human\n' \
        "$ISO" "$PUSHED" "$ERRORS" "$NEEDS_HUMAN" >>"$JOURNAL"
fi

# Propagate non-zero exit only for actual push errors
if [[ "$DRAIN_EXIT" -ne 0 ]]; then
    exit "$DRAIN_EXIT"
fi
exit 0
