#!/usr/bin/env bash
# Stop hook: advisory multi-model review at session end.
# Runs scripts/review.sh over the branch diff vs the default branch (master).
# Only the CLI reviewers run here — codex; the Claude `code-reviewer` subagent is
# shell-unreachable (review.sh skips it in shell context). Advisory reviewers
# never block; a reviewer marked blocking:true in harness.config.json will.
set -uo pipefail

INPUT=$(cat)
# Loop guard: we already ran a Stop hook this turn.
[ "$(echo "$INPUT" | jq -r '.stop_hook_active // false')" = "true" ] && exit 0

HOOK_DIR="$(cd "$(dirname "$0")" && pwd)"
SCRIPTS="$(cd "$HOOK_DIR/../../scripts" && pwd)"

# Locate harness.config.json (walk up from cwd) and run review from there.
dir="$(pwd)"
while [ -n "$dir" ] && [ "$dir" != "/" ]; do
  [ -f "$dir/harness.config.json" ] && break
  dir="$(dirname "$dir")"
done
[ -f "$dir/harness.config.json" ] || exit 0
cd "$dir" || exit 0

# codex is the only shell-runnable reviewer — skip the hook entirely if absent
# (keeps the gate a silent no-op on machines without codex).
command -v codex >/dev/null 2>&1 || exit 0

# This repo's default branch is master (review.sh defaults to origin/main).
BASE=$(git merge-base HEAD origin/master 2>/dev/null \
    || git merge-base HEAD master 2>/dev/null \
    || echo HEAD~1)

bash "$SCRIPTS/review.sh" "$BASE"
rc=$?
if [ "$rc" -eq 2 ]; then
  echo "Blocking reviewer reported findings — address them before finishing." >&2
  exit 2
fi
exit 0
