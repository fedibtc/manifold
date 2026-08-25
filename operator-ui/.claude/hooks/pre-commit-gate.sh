#!/usr/bin/env bash
# PreToolUse (Bash): when the command is a git commit, run the full staged gate.
set -uo pipefail

INPUT=$(cat)
COMMAND=$(echo "$INPUT" | jq -r '.tool_input.command // empty')
echo "$COMMAND" | grep -qE '(^|[;&|]\s*)git\s+commit' || exit 0

HOOK_DIR="$(cd "$(dirname "$0")" && pwd)"
GATE="$HOOK_DIR/../../scripts/gate.sh"

if ! bash "$GATE" --staged; then
  echo "Commit blocked by quality gate — resolve the findings above, restage, and retry." >&2
  exit 2
fi
exit 0
