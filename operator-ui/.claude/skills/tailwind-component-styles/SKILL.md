---
name: tailwind-component-styles
description: Create or refactor React components using Tailwind CSS v3 + colocated CSS Modules, @layer components, and evidence-based shared extraction. Use whenever TSX UI or CSS is added or changed in operator-ui.
---

# Tailwind component styling workflow (v3 + CSS Modules)

Apply the project rules in `.claude/rules/tailwind-css.md`. Core invariant:
**no Tailwind utility strings in TSX, ever** — `className` holds only `styles.*`
refs (or a merged caller `className`).

## 1. Inspect before editing

- Read the app CSS entry (`apps/<app>/src/app/app.css` or `src/index.css`) and
  `packages/shared-ui/tailwind-preset.cjs` for available tokens.
- Search the feature dir and ancestors for existing modules, shared
  `styles/utilities.css`, tokens, and equivalent declarations.
- Inspect the component's sibling files and imports.

## 2. Choose the correct abstraction

1. Existing Tailwind utility (via `@apply`)
2. Existing preset token (`text-ink`, `bg-surface`, `rounded-card`, …)
3. Existing shared `@layer utilities` class
4. Local component class in a CSS module ← the default
5. New preset token for a repeated design value
6. New shared utility class for a repeated single-purpose primitive
7. Shared React component for repeated semantic/multi-part UI

Exact duplication is evidence to review, not proof an abstraction is correct.

## 3. Create or update the component

For a styled component `Thing.tsx`:

- Create/update `Thing.module.css`, import it as `styles`.
- Move ALL visual styling out of TSX into the module — including single
  utilities like `mt-2`. No string literals or style consts remain in the TSX.
- Preserve accessibility attributes, semantics, behavior, public props.
- Keep caller overrides possible via an optional `className` prop when sensible.

v3 module shape — **bare `@apply`, no `@layer`/`@reference`** (a v3 module has no
`@tailwind` context, so `@layer components` fails to build):

```css
.root {
  @apply flex flex-col gap-4 rounded-card bg-surface p-6 text-ink;
}

.row {
  @apply flex items-center gap-2 hover:opacity-90 md:gap-4;
}
```

Gotchas: never name a class the same as a utility it applies (`.grid { @apply
grid }` → circular build error; use `.statGrid`). Opacity modifiers
(`text-ink/50`) work only on alpha-aware preset tokens (`ink`, `muted`).

Conditional styling → module refs or data attributes, never interpolated
strings:

```tsx
<div className={active ? styles.active : styles.idle} />
```

## 4. Handle duplication

Run `pnpm exec fallow dupes`. For each new duplicate:

- Repeated value → preset token.
- Repeated single-purpose primitive → shared `@layer utilities` class in the
  nearest shared `styles/utilities.css`, imported by the app CSS entry.
- Semantic UI → extract/reuse a shared React component (shared-ui components use
  preset tokens + shared-ui styles only, never an app's CSS).
- Coincidental/context-specific → leave local, explain in the summary.

## 5. Verify

- `node scripts/check-styles.mjs` — must report no violations.
- `pnpm exec fallow dupes` — resolve new CSS duplication.
- Typecheck + relevant tests.
- Review the diff for raw utility strings in TSX (there must be none),
  accidental global selectors, and unnecessary abstractions.
- Report which styles stayed local, which became tokens/utilities, and why.
