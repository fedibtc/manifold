---
paths:
  - "apps/**/*.tsx"
  - "apps/**/*.css"
  - "packages/shared-ui/**/*.tsx"
  - "packages/shared-ui/**/*.css"
---

# Tailwind CSS rules (v3 + CSS Modules, external-CSS convention)

We run **Tailwind CSS v3.4** with **CSS Modules**. Utility strings never appear
in TSX. This is enforced deterministically by `scripts/check-styles.mjs`
(harness `style` gate + Claude Code Stop hook).

## The hard rule: no Tailwind strings in TSX

- `className` may contain **only** a `styles.*` reference or a caller-provided
  `className` (optionally merged). Nothing else.
- **No string literals in `className`** — not inline, not braced, not in a
  ternary, not hoisted into a `const`. Zero exceptions. A single utility like
  `mt-2` still goes in the module.
- **No style consts.** `const rowClass = 'flex gap-2'` then `className={rowClass}`
  is a violation — the utility string is still in the TSX file.
- Do not add a class-name helper dependency unless it already exists.

Correct:

```tsx
import styles from './Thing.module.css';
export const Thing = ({ className }: { className?: string }) => (
  <div className={styles.root}>
    <span className={styles.label}>…</span>
  </div>
);
```

## Component ownership

- A production TSX component that owns styles has a sibling
  `ComponentName.module.css`, imported as `styles`.
- **No module needed when a component owns no styles** — providers, contexts,
  route guards, thin route files, layouts that only compose. Do not create empty
  modules.
- Exempt tests, specs, stories, and generated files.
- A component may accept and merge a caller `className`, but its own styling
  stays in its module.

## CSS module structure (v3)

- **Bare `@apply`, no `@layer`.** A v3 CSS module is processed in isolation with
  no `@tailwind` context, so `@layer components { … }` fails to build (`no
  matching @tailwind components directive`). Write top-level rules with `@apply`
  directly. (`@reference`/`@theme`/`@utility` are v4; `@layer` is valid v3 but
  needs the `@tailwind` context a module lacks — either way, keep them out of
  modules.)
- Prefer short local names: `.root`, `.header`, `.body`, `.footer`. CSS Modules
  already scope names.
- **Never name a class the same as a utility it applies** — `.grid { @apply
  grid }` is a build-time circular dependency. Use `.statGrid`, `.layout`, etc.
- Use `@apply` to compose Tailwind utilities.
- Use Tailwind variant prefixes for state/responsive/dark inside `@apply`
  (e.g. `@apply hover:opacity-90 md:grid-cols-3`), or native CSS where clearer.
- Prefer preset theme tokens (`text-ink`, `bg-surface`, `border-muted`,
  `text-status-healthy`, `rounded-card`, …) over hardcoded values. Opacity
  modifiers (`text-ink/50`, `border-muted/50`) work only on alpha-aware tokens —
  `ink` and `muted` are defined that way in the preset; other tokens are not.
- **Never** declare `@theme`, `@utility`, `@reference`, or `@layer` in a
  component module — `@theme`/`@utility`/`@reference` are v4, and `@layer` needs
  the `@tailwind` context a module lacks. Design tokens live in the shared preset
  (`packages/shared-ui/tailwind-preset.cjs`); custom utilities go in a shared
  utilities file (below).
- Avoid `!important`, deep selectors, and nesting beyond two levels.

Shape:

```css
.root {
  @apply flex flex-col gap-4 rounded-card bg-surface p-6 text-ink;
}

.row {
  @apply flex items-center gap-2 hover:opacity-90;
}
```

## Conditional styling — no string interpolation

- Map props/states to **complete, statically discoverable** `styles.*` refs:
  `className={active ? styles.active : styles.idle}` (each branch is a module
  ref, not a utility string).
- Prefer a data attribute + variant selector in the module for state
  (`&[data-active='true'] { @apply … }`).
- Never build class names with template interpolation.
- Inline `style={{}}` only for genuinely runtime values (a computed width, a
  chart color) that cannot be known at build time.

## Sharing ladder (mirrors §10 import direction, for CSS)

`component module → feature utilities → app shared/styles → shared-ui`. One-way,
same as code. Rule of three applies — don't pre-promote.

1. **Repeated design value** → add/reuse a token in
   `packages/shared-ui/tailwind-preset.cjs` (Fedi DS names).
2. **Repeated single-purpose primitive used by 2+ features in one app** →
   `@utility`-style class in `apps/<app>/src/shared/styles/utilities.css` under
   `@layer utilities`, **loaded into Tailwind by that app's `tailwind.config.cjs`
   (via `load-utilities.cjs`)** — not imported as CSS. Shared utility classes in
   these files must use a `u` prefix such as `uLabel`, `uIntro`, `uTableWrap`,
   `uCenteredPage`.
3. **Repeated primitive used by 2+ apps** → `packages/shared-ui/styles/`,
   **loaded into Tailwind by the shared preset (`tailwind-preset.cjs`)** — not
   imported as CSS.
4. **Repeated semantic/multi-part UI** (button, card, field, badge, modal) →
   a shared React component in `packages/shared-ui/src/components/` with its own
   module. **shared-ui components never reference an app's CSS** — they use the
   preset tokens and shared-ui styles only.
5. Leave intentional context-specific duplication local; note why.

Custom utility classes (`@layer utilities { .x { @apply … } }`):

- These `utilities.css` dictionaries are **loaded into the Tailwind config**
  (`load-utilities.cjs` registers each class as a component), not `@import`ed as
  CSS. That's load-bearing: a `.module.css` is compiled in isolation, so
  `@apply uPageHeading` can only resolve if the class is registered in the
  config — an `@import`ed `@layer` file is invisible to it in dev (it works only
  in a full build, where the whole CSS graph is compiled together). Tailwind's
  docs prescribe exactly this (define custom classes via the plugin system
  rather than `@apply`-ing across isolated files).
- Shared utility classes in app-level `utilities.css` must use the `u` prefix.
- CSS modules may keep natural local names like `.label` or `.intro`, then
  consume the shared utility with `@apply uLabel` or `@apply uIntro`.
- Name by reusable purpose, not by the first component that used it.
- Don't create a utility from a single occurrence, or to alias an existing
  Tailwind utility, or merely to cut lines.

## Required review after touching TSX or CSS

1. Inspect the diff for any string literal in a TSX `className` (including
   consts). There must be none.
2. Run `node scripts/check-styles.mjs` (or let the `style` gate / Stop hook run
   it).
3. Run `pnpm exec fallow dupes` when styling changed materially; resolve new CSS
   duplication with the ladder above.
4. Run typecheck and relevant tests.
