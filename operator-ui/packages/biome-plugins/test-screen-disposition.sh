#!/usr/bin/env bash
# Verifies the screen-query-disposition GritQL plugin.
#
# The rule's whole value is where it fires and where it does not, so each case
# places the SAME fixture at a different path in a sandbox that carries the real
# biome.json plugin entry, and asserts the expected number of diagnostics.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
biome="$root/node_modules/.bin/biome"
[ -x "$biome" ] || biome="$(command -v biome)"

fixtures="$here/__fixtures__/screen-disposition"
sandbox="$(mktemp -d)"
trap 'rm -rf "$sandbox"' EXIT
mkdir -p "$sandbox/packages/biome-plugins"
cp "$here/screen-query-disposition.grit" "$sandbox/packages/biome-plugins/"

# The `includes` glob is lifted from the REAL biome.json rather than restated
# here. A plugin glob anchored at `apps/…` instead of `**/apps/…` matches no
# file and the rule silently never fires, which a hardcoded copy would hide.
#
# Formatter off and linter rules off: the only diagnostics counted are the
# plugin's, so a fixture's formatting can never be mistaken for a rule hit.
node -e '
  const { readFileSync, writeFileSync } = require("node:fs");
  const [configPath, out] = process.argv.slice(1);
  const config = JSON.parse(readFileSync(configPath, "utf8"));
  const entry = (config.plugins ?? []).find(
    (plugin) => typeof plugin === "object" && plugin.path.endsWith("screen-query-disposition.grit")
  );
  if (!entry) throw new Error(`${configPath} registers no screen-query-disposition plugin`);
  writeFileSync(
    out,
    JSON.stringify({
      formatter: { enabled: false },
      linter: { enabled: true, rules: { recommended: false } },
      plugins: [{ path: entry.path, includes: entry.includes }]
    })
  );
' "$root/biome.json" "$sandbox/biome.json"

pass=0
fail=0

# run_case <name> <fixture> <path-under-sandbox> <expected-hits>
run_case () {
  local name="$1" fixture="$2" target="$3" expected="$4"
  mkdir -p "$sandbox/$(dirname "$target")"
  cp "$fixtures/$fixture" "$sandbox/$target"
  local output hits
  output="$(cd "$sandbox" && "$biome" check "$target" 2>&1 || true)"
  hits="$(printf '%s\n' "$output" | grep -c 'may not branch' || true)"
  rm -f "$sandbox/$target"

  if [ "$hits" -ne "$expected" ]; then
    echo "FAIL $name: expected $expected diagnostic(s), got $hits"
    printf '%s\n' "$output"
    fail=$((fail + 1))
    return
  fi
  # A diagnostic nobody can act on is not a gate: it has to name the file.
  if [ "$expected" -gt 0 ] && ! printf '%s\n' "$output" | grep -q "$target"; then
    echo "FAIL $name: diagnostic does not name $target"
    printf '%s\n' "$output"
    fail=$((fail + 1))
    return
  fi
  echo "PASS $name"
  pass=$((pass + 1))
}

run_case 'screen gated on isError' \
  gated-page.tsx apps/fleet-manager/src/pages/gated/GatedPage.tsx 1
run_case 'screen rendering through the surface' \
  disposed-page.tsx apps/fleet-manager/src/pages/disposed/DisposedPage.tsx 0
run_case 'a screens own unit test' \
  gated-page.tsx apps/fleet-manager/src/pages/gated/__tests__/GatedPage.test.tsx 0
run_case 'a mutation error in a feature component' \
  save-action.tsx apps/fleet-manager/src/features/offer/components/save-action/SaveAction.tsx 0
run_case 'the same violation in a feature component' \
  gated-page.tsx apps/fleet-manager/src/features/seats/components/seat-table/SeatTable.tsx 0
run_case 'another apps screen, which has no primitive' \
  gated-page.tsx apps/liquidity-provider/src/pages/funds/FundsPage.tsx 0

echo "----"
echo "passed: $pass  failed: $fail"
[ "$fail" -eq 0 ]
