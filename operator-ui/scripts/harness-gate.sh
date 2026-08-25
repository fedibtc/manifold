#!/usr/bin/env bash
# agent-harness gate adapter.
#
# The harness runs gates at the git-worktree root with shell:false and whitespace-
# split args, so gates cannot cd or chain. operator-ui is a subdir of the
# decentralized-federations monorepo, so each gate is invoked as:
#   bash operator-ui/scripts/harness-gate.sh <stage>
# which parses cleanly (executable=bash, args=[script, stage]) and lets this
# script cd into operator-ui and run the real check.
#
# Stages: install | typecheck | lint | boundaries | style | structure | fallow | test | all
set -euo pipefail

stage="${1:-all}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"  # operator-ui root
cd "$here"

# Use `pnpm run <script>` explicitly: bare `pnpm ci` collides with pnpm's built-in
# clean-install command and would silently skip the biome `ci` script.
run_install() { pnpm install --frozen-lockfile; }
run_typecheck() { pnpm -r run typecheck; }
run_lint() { pnpm run ci; }
# Architecture layering (shared <- features <- pages/app). Separate from `lint`
# because biome does not know about import layers and eslint runs only this rule.
run_boundaries() { pnpm run lint:boundaries; }
run_style() { node scripts/check-styles.mjs; }
run_structure() { node scripts/check-structure.mjs; }
run_fallow() { pnpm exec fallow dead-code && pnpm exec fallow dupes; }
run_test() { pnpm -r run test; }

case "$stage" in
  install) run_install ;;
  typecheck) run_typecheck ;;
  lint) run_lint ;;
  boundaries) run_boundaries ;;
  style) run_style ;;
  structure) run_structure ;;
  fallow) run_fallow ;;
  test) run_test ;;
  all) run_install && run_typecheck && run_lint && run_boundaries && run_style && run_structure && run_fallow && run_test ;;
  *) echo "unknown stage: $stage (want install|typecheck|lint|boundaries|style|structure|fallow|test|all)" >&2; exit 2 ;;
esac
