# Folder structure (feature-based layout)

Enforced by the `structure` harness gate and a Stop hook
(`scripts/check-structure.mjs`). These are the layout rules that ESLint
boundaries (import layers) and biome (formatting/lint) do **not** catch.

## Page folders hold only the page

`apps/*/src/pages/<feature>/` may contain **only**:

- `<X>Page.tsx` — the page component
- `<X>Page.module.css` — its colocated styles
- `__tests__/<X>Page.test.tsx`

No subcomponents, no utils, no extra subfolders. A page composes feature
components (the `pages` boundary layer may import `feature`); the pieces it
composes live in the feature, not in the page folder. `OverviewPage` is the
reference.

## Feature components live in kebab folders with a test

Every component under `apps/*/src/features/<feature>/components/**` must:

- sit in its **own kebab-case folder**: `components/<kebab>/PascalName.tsx`
- keep its styles colocated: `components/<kebab>/PascalName.module.css`
- have a **unit test**: `components/<kebab>/__tests__/PascalName.test.tsx`

No loose components directly in `components/`, and no untested components. Utils
go in `features/<feature>/utils/` (with their own `__tests__/`).

## One React unit (component or hook) per file

A file exports **at most one** React unit — a component (a PascalCase export in
a `.tsx`) **or** a hook (a `use*` export). Type/interface exports, plain
constants, and non-component/non-hook helper functions are **not** units, so:

- a component file exports one component (plus its `XProps` type — fine);
- a hook file exports one `use*` hook (plus a keys/const it owns — fine);
- a **utility file exports neither a component nor a hook**, so it is exempt by
  construction and may export any number of helpers.

Two components, two hooks, or a component + a hook in one file are all
violations — split the extra unit into its own file (a sibling kebab folder for
a component, its own hook file for a hook). This is enforced three ways: at
**create time** via a PostToolUse (Write/Edit) hook, on **Stop**, and in **CI**.

## Why a gate and not just a review

The convention is documented in `operator-ui/CLAUDE.md` ("File layout"), but a
generator that had that doc still flattened subcomponents into a page folder
with no tests. A deterministic gate blocks completion so the engineer
self-corrects in its own loop — the rule is enforced, not advisory.

```
pages/allocations/
  AllocationsPage.tsx
  AllocationsPage.module.css
  __tests__/AllocationsPage.test.tsx
features/allocations/
  components/allocation-status-chip/
    AllocationStatusChip.tsx
    AllocationStatusChip.module.css
    __tests__/AllocationStatusChip.test.tsx
  utils/
    formatSats.ts
    __tests__/formatSats.test.ts
```
