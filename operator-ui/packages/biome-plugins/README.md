# Biome GritQL plugins

## `jsx-sibling-blank-line.grit`

Enforces **exactly one blank line between adjacent sibling JSX elements** in
`.jsx` / `.tsx` files.

- Matches the `JsxText()` whitespace node between two siblings where BOTH the
  previous and next sibling are a `JsxElement()`, `JsxSelfClosingElement()`, or
  `JsxFragment()`.
- Fires only when that whitespace is a single newline (`\r?\n`) plus indent.
- Reports `error`: `Add a blank line between sibling JSX elements.`
- Provides a **safe** autofix: one newline → two, reusing the captured indent.
- Idempotent — an existing blank line already has two newlines, so it does not
  re-match.
- Never touches: a child next to its parent's closing tag (only one side is an
  element), text content, strings, template literals, or JS expressions
  (the match is anchored on a whitespace-only `JsxText` node with an element on
  each side).

Registered in [`../biome.json`](../../biome.json) under `plugins`, scoped to
`**/*.jsx` and `**/*.tsx`.

## `screen-query-disposition.grit`

Rejects a **fleet-manager screen branching on a query's raw `isError`**.

React-query keeps `data` through a failed refresh, so "we hold an answer" and
"the last attempt failed" are independent facts. A screen reading only `isError`
collapses them and says something false — it deletes figures it still holds, or
claims an empty fleet it was never told about. Six FMan screens each invented
their own answer to this before `useQueryDisposition` / `QuerySurface` existed
(`reviews/underwriting/issues/no-disposition-for-answered-then-failed.md`). The
rule is what stops a seventh.

- Matches any `$read.isError` member read; reports `error` with the message
  naming the replacement. No autofix — the correct shape is a judgement about
  what the screen claims, not a rewrite.
- Registered in [`../../biome.json`](../../biome.json) under `plugins`, scoped
  to `**/apps/fleet-manager/src/pages/**/*.tsx` minus `**/__tests__/**`.
  **The leading `**/` is load-bearing** — a plugin `includes` glob anchored at
  `apps/…` matches nothing, and the rule silently never fires.

Two exclusions, both deliberate:

- **Feature components are out of scope.** A *mutation's* `isError` is an action
  that failed, not a read the screen has to dispose of. Banning it there would
  fire on every Save button in the app, and a rule with false alarms gets
  switched off. The cost is real: a screen can still push the branch down into a
  feature hook and escape. That residual gap is recorded on the issue.
- **`apps/liquidity-provider` is out of scope.** The disposition primitive is an
  FMan module; a rule may only require what exists. FLIP needs its own copy of
  the primitive before it can be held to the same standard.

## Fixtures & tests

`__fixtures__/` is excluded from the workspace check (`files.includes` in
`biome.json`) because the fixtures intentionally violate the rules.

```sh
pnpm plugin:test   # runs both suites
```

- `test-fixtures.sh` — `<name>.input.tsx` / `<name>.expected.tsx` pairs for the
  blank-line plugin (fix + idempotency, each in an isolated sandbox config, plus
  an inline CRLF case). Cases: basic siblings, self-closing siblings, fragments,
  nested trees, existing blank line (unchanged), child-before-parent-close
  (unchanged), inline text mixed with JSX (unchanged), CRLF line endings.
- `test-screen-disposition.sh` — the disposition rule is about *where* it fires,
  so each case copies a fixture from `__fixtures__/screen-disposition/` to a
  different path in a sandbox carrying the real plugin entry and asserts the
  diagnostic count. Cases: a gated screen (1 hit, and the diagnostic must name
  the file), a screen rendering through `QuerySurface` (0), a screen's own unit
  test (0), a mutation error in a feature component (0), the same violation in a
  feature component (0), and another app's screen (0).
