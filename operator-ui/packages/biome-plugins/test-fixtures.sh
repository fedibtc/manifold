#!/usr/bin/env bash
# Verifies the jsx-sibling-blank-line GritQL plugin against the fixtures in
# __fixtures__/. Each `<name>.input.tsx` is fixed with `biome check --write` in
# an isolated sandbox and compared byte-for-byte against `<name>.expected.tsx`.
# Idempotency is asserted by re-running `biome check` and expecting no findings.
# A CRLF case is generated inline (git would normalise a committed CRLF file).
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/.." && pwd)"
biome="$root/node_modules/.bin/biome"
[ -x "$biome" ] || biome="$(command -v biome)"

sandbox="$(mktemp -d)"
crlfbox="$(mktemp -d)"
trap 'rm -rf "$sandbox" "$crlfbox"' EXIT
mkdir -p "$sandbox/biome-plugins"
cp "$here/jsx-sibling-blank-line.grit" "$sandbox/biome-plugins/"

cat > "$sandbox/biome.json" <<'JSON'
{
  "$schema": "https://biomejs.dev/schemas/2.5.2/schema.json",
  "formatter": { "enabled": true, "indentStyle": "space", "indentWidth": 2, "lineWidth": 100 },
  "linter": { "enabled": true, "rules": { "recommended": true, "suspicious": { "noUnknownAtRules": "off" } } },
  "javascript": { "formatter": { "quoteStyle": "single", "trailingCommas": "none" } },
  "plugins": [{ "path": "./biome-plugins/jsx-sibling-blank-line.grit", "includes": ["**/*.jsx", "**/*.tsx"] }]
}
JSON

pass=0
fail=0

run_case () {
  local name="$1" input="$2" expected="$3"
  local work="$sandbox/$name.tsx"
  cp "$input" "$work"
  ( cd "$sandbox" && "$biome" check --write "$name.tsx" >/dev/null 2>&1 || true )
  if ! diff -u "$expected" "$work"; then
    echo "FAIL $name: fix output does not match expected"; fail=$((fail+1)); return
  fi
  # idempotency: no findings on the already-fixed file
  if ! ( cd "$sandbox" && "$biome" check "$name.tsx" >/dev/null 2>&1 ); then
    echo "FAIL $name: not idempotent (findings remain after fix)"; fail=$((fail+1)); return
  fi
  echo "PASS $name"; pass=$((pass+1))
}

for input in "$here/__fixtures__"/*.input.tsx; do
  name="$(basename "$input" .input.tsx)"
  run_case "$name" "$input" "$here/__fixtures__/$name.expected.tsx"
done

# CRLF case (generated: single CRLF newline between siblings -> two CRLFs).
# Dedicated sandbox with lineEnding "crlf" so the formatter does not normalise
# CRLF -> LF and mask what the plugin actually produced.
mkdir -p "$crlfbox/biome-plugins"
cp "$here/jsx-sibling-blank-line.grit" "$crlfbox/biome-plugins/"
cat > "$crlfbox/biome.json" <<'JSON'
{
  "$schema": "https://biomejs.dev/schemas/2.5.2/schema.json",
  "formatter": { "enabled": true, "indentStyle": "space", "indentWidth": 2, "lineWidth": 100, "lineEnding": "crlf" },
  "linter": { "enabled": true, "rules": { "recommended": true, "suspicious": { "noUnknownAtRules": "off" } } },
  "javascript": { "formatter": { "quoteStyle": "single", "trailingCommas": "none" } },
  "plugins": [{ "path": "./biome-plugins/jsx-sibling-blank-line.grit", "includes": ["**/*.jsx", "**/*.tsx"] }]
}
JSON
printf 'export const Crlf = () => (\r\n  <div>\r\n    <span>One</span>\r\n    <span>Two</span>\r\n  </div>\r\n);\r\n' > "$crlfbox/crlf.tsx"
printf 'export const Crlf = () => (\r\n  <div>\r\n    <span>One</span>\r\n\r\n    <span>Two</span>\r\n  </div>\r\n);\r\n' > "$crlfbox/crlf.expected"
( cd "$crlfbox" && "$biome" check --write "crlf.tsx" >/dev/null 2>&1 || true )
if diff -u "$crlfbox/crlf.expected" "$crlfbox/crlf.tsx" && ( cd "$crlfbox" && "$biome" check "crlf.tsx" >/dev/null 2>&1 ); then
  echo "PASS crlf"; pass=$((pass+1))
else
  echo "FAIL crlf"; fail=$((fail+1))
fi

echo "----"
echo "passed: $pass  failed: $fail"
[ "$fail" -eq 0 ]
