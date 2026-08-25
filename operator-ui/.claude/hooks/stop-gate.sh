#!/usr/bin/env bash
# Stop hook: final quality sweep over uncommitted work before Claude yields.
# Loop guard FIRST: stop_hook_active means we already blocked once this turn.
set -uo pipefail

INPUT=$(cat)
[ "$(echo "$INPUT" | jq -r '.stop_hook_active // false')" = "true" ] && exit 0

HOOK_DIR="$(cd "$(dirname "$0")" && pwd)"
GATE="$HOOK_DIR/../../scripts/gate.sh"

find_harness_config() {
  local dir="$1"
  while [ -n "$dir" ] && [ "$dir" != "/" ]; do
    if [ -f "$dir/harness.config.json" ]; then
      echo "$dir/harness.config.json"
      return 0
    fi
    dir="$(dirname "$dir")"
  done
  return 1
}

CWD="$(pwd)"
CHANGED=$(git diff --name-only --diff-filter=ACMR; git diff --cached --name-only --diff-filter=ACMR)
CHANGED=$(echo "$CHANGED" | sort -u | grep -Ev '^$' || true)
[ -z "$CHANGED" ] && exit 0

SEED=$(echo "$CHANGED" | head -1)
find_harness_config "$(cd "$(dirname "$SEED")" 2>/dev/null && pwd || echo "$CWD")" >/dev/null || {
  echo "harness: no harness.config.json for changed files — fail closed." >&2
  exit 2
}

BASE=$(git merge-base HEAD origin/main 2>/dev/null || echo HEAD)
if ! bash "$GATE" --changed "$BASE"; then
  echo "Work does not pass the quality gate yet — fix the findings above before finishing." >&2
  exit 2
fi
exit 0
