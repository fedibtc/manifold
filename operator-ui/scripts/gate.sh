#!/usr/bin/env bash
# gate.sh — THE quality gate. One script, three entry points:
#   Claude hooks (pre-commit + Stop) · lefthook git pre-commit · CI.
#
# Usage:
#   gate.sh --staged           # staged files (git pre-commit, Claude commit hook)
#   gate.sh --changed <base>   # diff vs base ref (Stop hook, CI on PRs)
#   gate.sh --all [--e2e]      # full sweep (CI main, on demand)
#
# Severity per check comes from harness.config.json → checks:
#   block | block-new | report | off
# Exit 2 on any blocking failure, with actionable stderr.
set -uo pipefail

MODE="${1:---staged}"
BASE="${2:-}"
START_CWD="$(pwd)"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

find_harness_config() {
  local start="$1" dir
  if [ -f "$start" ]; then dir="$(cd "$(dirname "$start")" && pwd)"
  elif [ -d "$start" ]; then dir="$(cd "$start" && pwd)"
  else dir="$(cd "$(dirname "$start")" 2>/dev/null && pwd || echo "$START_CWD")"
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

run_with_timeout() {
  if command -v timeout >/dev/null 2>&1; then
    timeout 300 "$@"
  else
    "$@"
  fi
}

case "$MODE" in
  --staged)  FILES=$(git diff --cached --name-only --diff-filter=ACMR) ;;
  --changed) FILES=$(git diff --name-only --diff-filter=ACMR "${BASE:-origin/main}"...HEAD 2>/dev/null; git diff --name-only --diff-filter=ACMR) ;;
  --all)     FILES=$(git ls-files) ;;
  *) echo "usage: gate.sh --staged | --changed <base> | --all [--e2e]" >&2; exit 1 ;;
esac
FILES=$(echo "$FILES" | sort -u | grep -Ev '^$' || true)
CODE_FILES=$(echo "$FILES" | grep -E '\.(ts|tsx|js|jsx|mjs)$' | grep -v node_modules || true)
SRC_FILES=$(echo "$CODE_FILES" | grep -Ev '\.(test|spec|e2e)\.' | grep -Ev '(^|/)(e2e|__mocks__|__fixtures__)/' || true)
TEST_FILES=$(echo "$CODE_FILES" | grep -E '(\.(test|spec|e2e)\.|(^|/)e2e/)' || true)

[ -z "$CODE_FILES" ] && { echo "gate: no code files in scope — pass."; exit 0; }

SEED_FILE=$(echo "$CODE_FILES" | head -1)
if [ -n "$SEED_FILE" ] && [ -e "$SEED_FILE" ]; then
  CONFIG_PATH="$(find_harness_config "$SEED_FILE")" || true
else
  CONFIG_PATH="$(find_harness_config "$START_CWD")" || true
fi
if [ -z "${CONFIG_PATH:-}" ]; then
  echo "gate: no harness.config.json found walking up from changed files — fail closed." >&2
  exit 2
fi
CONFIG_DIR="$(dirname "$CONFIG_PATH")"
CONFIG="$CONFIG_PATH"
GIT_ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"

rebase_paths() {
  local out="" f abs
  while IFS= read -r f; do
    [ -z "$f" ] && continue
    if [ -e "$f" ]; then abs="$(cd "$(dirname "$f")" && pwd)/$(basename "$f")"
    else abs="$GIT_ROOT/$f"
    fi
    out+="${abs#"$CONFIG_DIR"/}"$'\n'
  done <<< "$1"
  echo "$out" | grep -Ev '^$' || true
}

CODE_FILES=$(rebase_paths "$CODE_FILES")
SRC_FILES=$(rebase_paths "$SRC_FILES")
TEST_FILES=$(rebase_paths "$TEST_FILES")
cd "$CONFIG_DIR" || exit 1

severity() { node -p "try{JSON.parse(require('fs').readFileSync(process.argv[1],'utf8')).checks['$1']||'off'}catch{'off'}" "$CONFIG" 2>/dev/null || echo off; }
config_value() { node -p "try{JSON.parse(require('fs').readFileSync(process.argv[1],'utf8')).$1||''}catch{''}" "$CONFIG" 2>/dev/null || echo ''; }

FAILURES=()
REPORTS=()

run_check() {
  local NAME="$1"; shift
  local LEVEL; LEVEL="$(severity "$NAME")"
  [ "$LEVEL" = "off" ] && return 0
  local OUTPUT
  if OUTPUT=$(run_with_timeout "$@" 2>&1); then
    return 0
  fi
  if [ "$LEVEL" = "report" ]; then
    REPORTS+=("[$NAME]"$'\n'"$OUTPUT")
  else
    FAILURES+=("[$NAME]"$'\n'"$OUTPUT")
  fi
}

check_filenames() {
  local BAD=0 OUT=""
  while IFS= read -r FILE; do
    [ -z "$FILE" ] && continue
    if ! MSG=$(bash "$SCRIPT_DIR/check-filename.sh" "$FILE" 2>&1); then OUT+="$MSG"$'\n'; BAD=1; fi
  done <<< "$CODE_FILES"
  [ $BAD -eq 0 ] || { echo "$OUT"; return 1; }
}

check_eslint() {
  # shellcheck disable=SC2086
  npx --no-install eslint $CODE_FILES --no-error-on-unmatched-pattern || {
    echo "→ Fix the lint errors above; rules trace to docs/clean-code.md."; return 1; }
}

check_dispatch_grep() {
  local HITS
  # shellcheck disable=SC2086
  HITS=$(grep -Hn "Dispatch<SetStateAction" $SRC_FILES 2>/dev/null || true)
  [ -z "$HITS" ] || { echo "$HITS"; echo "→ Setter type crossing a boundary — expose onChange/onCommit instead (docs/clean-code.md §2)."; return 1; }
}

check_tests_accompany() {
  [ "${SKIP_TESTS_CHECK:-0}" = "1" ] && return 0
  [ -z "$SRC_FILES" ] && return 0
  [ -n "$TEST_FILES" ] || {
    echo "Source files changed with no test changes:"; echo "$SRC_FILES" | sed 's/^/  /'
    echo "→ Tests accompany code (docs/clean-code.md §8). Add/adjust tests, or SKIP_TESTS_CHECK=1 for genuine no-behavior changes (renames, comments)."; return 1; }
}

check_related_tests() {
  local CMD; CMD="$(config_value testCommand)"
  [ -z "$CMD" ] || [ -z "$SRC_FILES" ] && return 0
  # shellcheck disable=SC2086
  $CMD $SRC_FILES || { echo "→ Related tests failing — fix before proceeding."; return 1; }
}

check_duplication() {
  if npx --no-install fallow --version >/dev/null 2>&1; then
    npx --no-install fallow audit --format json || { echo "→ New duplication vs baseline. Extract (rule of three) or justify + refresh baseline: npx fallow dupes --save-baseline"; return 1; }
  elif npx --no-install jscpd --version >/dev/null 2>&1; then
    npx --no-install jscpd src --min-tokens 50 --exitCode 1 --silent || { echo "→ Duplication detected (jscpd). Extract or justify (docs/clean-code.md §4)."; return 1; }
  else
    echo "duplication: fallow/jscpd not installed — skipped." >&2
  fi
}

# Roots are apps/ and packages/: this workspace has no top-level src/ or app/.
# stderr is NOT discarded — a crashing checker must be visible, not silently green.
check_shapes() { node "$SCRIPT_DIR/jsx-shape-dupes.mjs" --fail apps packages; }
check_knip() { npx --no-install knip --no-exit-code >/dev/null 2>&1 && npx --no-install knip || true; npx --no-install knip 2>/dev/null; }
check_gitleaks() {
  command -v gitleaks >/dev/null 2>&1 || { echo "gitleaks not installed — skipped." >&2; return 0; }
  if [ "$MODE" = "--staged" ]; then gitleaks protect --staged --no-banner; else gitleaks detect --no-banner; fi
}
check_semgrep() {
  command -v semgrep >/dev/null 2>&1 || { echo "semgrep not installed — skipped." >&2; return 0; }
  # shellcheck disable=SC2086
  semgrep scan --config auto --error --quiet $CODE_FILES
}
check_ratchet() { node "$SCRIPT_DIR/ratchet.mjs" check; }

check_boundaries() {
  if node -e "const p=require('./package.json'); process.exit(p.scripts&&p.scripts['lint:boundaries']?0:1)" 2>/dev/null; then
    npm run -s lint:boundaries || { echo "→ Boundary lint failed."; return 1; }
  else
    echo "boundaries: lint:boundaries script missing — skipped." >&2
  fi
}

check_compiler() {
  if node -e "const p=require('./package.json'); process.exit(p.scripts&&p.scripts['lint:compiler']?0:1)" 2>/dev/null; then
    npm run -s lint:compiler || { echo "→ Compiler lint failed."; return 1; }
  else
    echo "compiler: lint:compiler script missing — skipped." >&2
  fi
}

run_check filename check_filenames
run_check eslint check_eslint
run_check dispatchPropGrep check_dispatch_grep
run_check testsAccompanyCode check_tests_accompany
run_check relatedTests check_related_tests
run_check duplication check_duplication
run_check jsxShapes check_shapes
run_check knip check_knip
run_check gitleaks check_gitleaks
run_check semgrep check_semgrep
run_check ratchet check_ratchet
run_check boundaries check_boundaries
run_check compiler check_compiler

if [[ "${*}" == *"--e2e"* ]]; then
  E2E_CMD="$(config_value e2eCommand)"
  [ -n "$E2E_CMD" ] && run_check e2e $E2E_CMD
fi

if [ ${#REPORTS[@]} -gt 0 ]; then
  echo "" >&2; echo "── gate: advisory findings (non-blocking) ──" >&2
  printf '%s\n\n' "${REPORTS[@]}" >&2
fi

if [ ${#FAILURES[@]} -gt 0 ]; then
  echo "" >&2; echo "── gate: BLOCKED — fix the following ──" >&2
  printf '%s\n\n' "${FAILURES[@]}" >&2
  exit 2
fi

echo "gate: all checks passed."
