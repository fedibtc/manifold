#!/usr/bin/env bash
# check-filename.sh <path> — enforces file naming (docs/clean-code.md §1):
#   components PascalCase.tsx · hooks useCamelCase.ts · folders kebab-case
# Exit 0 ok, exit 2 with actionable message on violation.
set -euo pipefail

FILE_PATH="${1:?usage: check-filename.sh <path>}"
BASE="$(basename "$FILE_PATH")"

# Framework-reserved names (Next.js app router etc.) are always allowed.
RESERVED='^(page|layout|loading|error|global-error|not-found|template|default|route|middleware|instrumentation|head|index)\.[jt]sx?$'
[[ "$BASE" =~ $RESERVED ]] && exit 0

# Config/dot/declaration files allowed.
[[ "$BASE" == *.config.* || "$BASE" == .* || "$BASE" == *.d.ts ]] && exit 0

# E2E specs are kebab-case, named for the user journey (e2e skill):
if [[ "$BASE" == *.e2e.ts || "$FILE_PATH" == e2e/* || "$FILE_PATH" == */e2e/* ]]; then
  STEM="${BASE%%.*}"
  [[ "$STEM" =~ ^[a-z0-9]+(-[a-z0-9]+)*$ ]] && exit 0
  echo "Filename violation: $FILE_PATH" >&2
  echo "  e2e specs are kebab-case named for the journey: checkout-with-valid-cart.e2e.ts" >&2
  exit 2
fi

fail() {
  echo "Filename violation: $FILE_PATH" >&2
  echo "  $1" >&2
  echo "  Convention: components PascalCase.tsx (CheckoutSummary.tsx) · hooks useCamelCase.ts (useCheckout.ts) · folders kebab-case (checkout-summary/) · other files camelCase.ts" >&2
  exit 2
}

STEM="${BASE%%.*}"
EXT="${BASE#"$STEM"}"

case "$EXT" in
  .tsx|.jsx|.test.tsx|.spec.tsx|.test.jsx|.spec.jsx|.stories.tsx)
    if [[ "$STEM" =~ ^use[A-Z] ]]; then
      [[ "$STEM" =~ ^use[A-Z][a-zA-Z0-9]*$ ]] || fail "hook files are camelCase starting with 'use': useCheckout.tsx"
    else
      [[ "$STEM" =~ ^[A-Z][a-zA-Z0-9]*$ ]] || fail "component files are PascalCase: $(echo "$STEM" | sed -E 's/(^|[-_])([a-z])/\U\2/g').tsx"
    fi
    ;;
  .ts|.js|.mjs|.test.ts|.spec.ts|.test.js|.spec.js|.e2e.ts)
    [[ "$STEM" =~ ^(use)?[a-z][a-zA-Z0-9]*$ ]] || fail "non-component files are camelCase: $(echo "$STEM" | sed -E 's/[-_]([a-z])/\U\1/g; s/^([A-Z])/\L\1/').ts"
    ;;
esac

# Folder segments must be kebab-case (allow __tests__/__mocks__/__fixtures__,
# Next.js [param], (group), @slot, and dotdirs).
DIR_PART="$(dirname "$FILE_PATH")"
IFS='/' read -ra SEGMENTS <<< "$DIR_PART"
for SEGMENT in "${SEGMENTS[@]}"; do
  [[ -z "$SEGMENT" || "$SEGMENT" == "." || "$SEGMENT" == ".." ]] && continue
  [[ "$SEGMENT" =~ ^(__|\.|\[|\(|@) ]] && continue
  [[ "$SEGMENT" =~ ^(src|app|pages|public|e2e|node_modules)$ ]] && continue
  [[ "$SEGMENT" =~ ^[a-z0-9]+(-[a-z0-9]+)*$ ]] || fail "folder '$SEGMENT' must be kebab-case: $(echo "$SEGMENT" | sed -E 's/([A-Z])/-\L\1/g; s/^-//; s/_/-/g')"
done

exit 0
