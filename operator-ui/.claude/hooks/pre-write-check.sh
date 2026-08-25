#!/usr/bin/env bash
# PreToolUse (Write|Edit): filename convention + protected-dirs deny. Zero tokens.
set -uo pipefail

INPUT=$(cat)
FILE_PATH=$(echo "$INPUT" | jq -r '.tool_input.file_path // empty')
TOOL=$(echo "$INPUT" | jq -r '.tool_name // empty')
[ -z "$FILE_PATH" ] && exit 0

HOOK_DIR="$(cd "$(dirname "$0")" && pwd)"
HARNESS_SCRIPTS="$(cd "$HOOK_DIR/../../scripts" && pwd)"

find_harness_config() {
  local start="$1" dir
  if [ -f "$start" ]; then dir="$(cd "$(dirname "$start")" 2>/dev/null && pwd || dirname "$start")"
  else
    dir="$(dirname "$start")"
    while [ ! -d "$dir" ] && [ "$dir" != "/" ]; do dir="$(dirname "$dir")"; done
    dir="$(cd "$dir" 2>/dev/null && pwd || echo "$dir")"
  fi
  while [ -n "$dir" ] && [ "$dir" != "/" ]; do
    if [ -f "$dir/harness.config.json" ]; then
      echo "$dir/harness.config.json"
      return 0
    fi
    dir="$(dirname "$dir")"
  done
  return 1
}

CONFIG_PATH="$(find_harness_config "$FILE_PATH")" || {
  echo "harness: no harness.config.json above $FILE_PATH — fail closed." >&2
  exit 2
}
CONFIG_DIR="$(dirname "$CONFIG_PATH")"
RELATIVE_PATH="${FILE_PATH#"$CONFIG_DIR"/}"

# Protected-dirs denial lives in the agent-toolkit standards-hooks plugin, which
# reads agent-toolkit.json — the single source for that list. That hook resolves the
# repo root from the edited file, so it also fires when Claude runs from the monorepo
# root, where this project-local hook is not loaded at all. Duplicating it here meant
# two lists to keep in step, and the copy read the key unguarded: a missing
# protectedDirs threw, stderr was discarded, and the guard vanished with no message.

if [ "$TOOL" = "Write" ] && [ ! -f "$FILE_PATH" ]; then
  case "$RELATIVE_PATH" in
    *.ts|*.tsx|*.js|*.jsx)
      bash "$HARNESS_SCRIPTS/check-filename.sh" "$RELATIVE_PATH" || exit 2 ;;
  esac
fi
exit 0
