#!/usr/bin/env bash
# review.sh — pluggable multi-model review. Runs every enabled CLI reviewer in
# harness.config.json over the diff vs <base>, printing PER-REVIEWER sections
# that are never merged (one reviewer must not mask another).
#
# Usage: review.sh [base-ref]   (default: merge-base with origin/main)
set -uo pipefail

BASE="${1:-$(git merge-base HEAD origin/main 2>/dev/null || echo HEAD~1)}"
git rev-parse --verify "$BASE" >/dev/null 2>&1 || { echo "review: bad base ref '$BASE'" >&2; exit 1; }
[ -z "$(git diff "$BASE"...HEAD 2>/dev/null; git diff)" ] && { echo "review: empty diff vs $BASE — nothing to review."; exit 0; }

CONFIG="harness.config.json"
COUNT=$(node -p "JSON.parse(require('fs').readFileSync('$CONFIG','utf8')).reviewers.length")
BLOCKED=0

for ((i = 0; i < COUNT; i++)); do
  NAME=$(node -p "JSON.parse(require('fs').readFileSync('$CONFIG','utf8')).reviewers[$i].name")
  TYPE=$(node -p "JSON.parse(require('fs').readFileSync('$CONFIG','utf8')).reviewers[$i].type")
  ENABLED=$(node -p "String(JSON.parse(require('fs').readFileSync('$CONFIG','utf8')).reviewers[$i].enabled ?? true)")
  BLOCKING=$(node -p "String(JSON.parse(require('fs').readFileSync('$CONFIG','utf8')).reviewers[$i].blocking ?? false)")

  [ "$ENABLED" != "true" ] && continue

  echo ""
  echo "════════ Reviewer: $NAME ════════"

  case "$TYPE" in
    subagent)
      echo "(runs inside Claude Code — invoke the '$(node -p "JSON.parse(require('fs').readFileSync('$CONFIG','utf8')).reviewers[$i].agent")' subagent; skipped in shell context)"
      ;;
    cli)
      CMD=$(node -p "JSON.parse(require('fs').readFileSync('$CONFIG','utf8')).reviewers[$i].command")
      CMD="${CMD//\{base\}/$BASE}"
      FIRST_WORD="${CMD%% *}"
      if ! command -v "$FIRST_WORD" >/dev/null 2>&1; then
        echo "($FIRST_WORD not installed — skipped)"
        continue
      fi
      if ! bash -c "$CMD"; then
        [ "$BLOCKING" = "true" ] && BLOCKED=1
        echo "($NAME reported findings$([ "$BLOCKING" = "true" ] && echo ' — BLOCKING' || echo ' — advisory'))"
      fi
      ;;
    external)
      echo "(external PR reviewer — runs via its GitHub App on pull requests; nothing to do locally)"
      ;;
  esac
done

echo ""
[ $BLOCKED -eq 1 ] && { echo "review: a blocking reviewer failed." >&2; exit 2; }
echo "review: complete (advisory reviewers never block)."
