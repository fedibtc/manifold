# operator-ui — component & test conventions

Rules for all frontend code under `operator-ui/` (`apps/*`, `packages/*`).
Applies to Claude and human contributors.

## Design basis (authoritative — always)

All UI **must** be based on:

1. **MVP wireframes** — [`docs/operator-dashboards/liquidity-manager/mvp/wireframes/`](../docs/operator-dashboards/liquidity-manager/mvp/wireframes/) (structure, layout, copy, states). Match pages 1:1.
2. **Fedi Design System** — the `fedi-design` MCP (tokens, colors, typography, components) and the Fedi Lab reference at https://fedi-lab.vercel.app/mcp. Tokens/colors/radii/shadows come from here only — no hardcoded values, no other UI kit. Run `check_against_system` on new UI before marking done.

The wireframes give the *what* (pages/flows); the Fedi Design System gives the *look* (tokens/components). When they disagree, the design system wins on styling, the wireframes win on structure/behavior.

> Assumptions taken where the source rule was ambiguous (correct any that are wrong):
>
> - "implicit/explicit return" → **implicit return only when the body is pure JSX**; any pre-return logic uses explicit `return`.
> - "absolute paths" → path alias **`@/`** per app `src/` + `@operator-ui/*` across packages. **Library-internal exception:** files inside `packages/common-ui` import siblings **relatively** — a single `@/` alias cannot point at both an app's `src/` and the library's `src/`, and cross-package path aliases are fragile.
> - Folder shape → component folder with a `__tests__/` subdir, **no barrel `index.ts`** per component (the package-level `src/index.ts` public export barrel stays).

## Components

- **Lambda declarations.** Declare components as arrow-function consts with a named export. No `function` declarations, no `React.FC`.
  ```tsx
  export const StatusBadge = (props: StatusBadgeProps) => { ... };
  ```
- **Return style.** If the component body is only a returned JSX tree, use an **implicit return**. If any logic or hooks run before the markup, use an **explicit `return`**.
  ```tsx
  // pure JSX → implicit
  export const Divider = () => <hr className={styles.divider} />;

  // has logic → explicit
  export const SeatRow = ({ seat }: SeatRowProps) => {
    const label = formatSeat(seat);
    return <tr>{label}</tr>;
  };
  ```
- **Declared Props type, always.** Every component declares a `XProps` interface. Never inline the prop object type in the parameter list.
  ```tsx
  // no
  export const Foo = (props: { title: string }) => ...
  // yes
  interface FooProps { title: string; }
  export const Foo = (props: FooProps) => ...
  ```
- **No inline logic in JSX.** Hoist handlers and derived values to a named `const` (or a hook). Genuinely-runtime inline `style={{}}` (a computed width/color unknown at build time) may stay; static styling never does — it goes in the CSS module (see **Styling**).
  ```tsx
  // no
  <div onClick={() => doThing(x)} />;
  // yes
  const handleClick = () => doThing(x);
  <div className={styles.card} onClick={handleClick} />;
  ```
- **Minimal comments.** Names carry intent; drop comments that restate the code. Keep only comments that explain non-obvious _why_.

## File layout

> Enforced by the `structure` harness gate + Stop hook + a PostToolUse (Write/Edit) create-time hook (`scripts/check-structure.mjs`); full rules in [`.claude/rules/folder-structure.md`](.claude/rules/folder-structure.md). A page folder holds only `<X>Page.tsx` + `.module.css` + `__tests__/`; every feature component lives in `features/<feature>/components/<kebab>/` with a colocated module and a `__tests__/` unit test.

- **One React unit (component or hook) per file.** A `.tsx` exports exactly one component; a hook file exports exactly one `use*` hook. Type/const/plain-helper exports do not count — so a utility file (which exports neither a component nor a hook) may export as many helpers as it likes. Split a second component or hook into its own file. Enforced at create time (PostToolUse) as well as on Stop/CI.
- **One component per kebab-case folder; file name is PascalCase.** The folder is kebab-case, the component file is `PascalCase.tsx` matching the exported component: `components/status-badge/StatusBadge.tsx`.
- **Colocate hooks** in the same folder, camelCase file: `components/status-badge/useStatus.ts`.
- **Tests in a `__tests__/` subfolder**, PascalCase file matching the component: `components/status-badge/__tests__/StatusBadge.test.tsx`.
  - Exception: **e2e tests** live at app level in `apps/*/e2e/` (Playwright), not in `__tests__/`.
- **Absolute imports only (app code).** Never relative (`../`, `./sibling`) in `apps/*`. Use the `@/` alias within an app and `@operator-ui/*` across packages. Exception: `packages/common-ui` internals import siblings relatively (see assumptions).

```text
apps/flip/src/components/
  status-badge/           # folder: kebab-case
    StatusBadge.tsx       # component: PascalCase
    useStatus.ts          # hook: camelCase
    __tests__/
      StatusBadge.test.tsx  # test: PascalCase
```

## Tests

- **Vitest, `it("should …")`.** Every `it` block description starts with `should`.
  ```ts
  it("should render the human label for a health status", () => { ... });
  ```
- **Restore overridden globals.** If a test overrides a global (e.g. `fetch`, timers, `window.*`), capture the original and restore it after the test (`afterEach`/`try…finally`). No leaked global state between tests.

## Query state on a screen (biome-enforced, fleet-manager)

> Enforced by `packages/biome-plugins/screen-query-disposition.grit` under
> `biome check .`; rationale and scope in
> [`packages/biome-plugins/README.md`](packages/biome-plugins/README.md).

- **A screen never branches on a query's raw `isError`.** React-query keeps
  `data` through a failed refresh, so "we hold an answer" and "the last attempt
  failed" are separate facts. Reading one of them makes the screen state
  something false — deleted figures, or an empty fleet nobody reported.
- Pass the screen's reads to `useQueryDisposition` and wrap every claim about
  the fleet in `QuerySurface`. Four states, no fifth: `loading`, `failed` (with
  a retry), `stale` (data kept under a dated banner), `content`.
- A **mutation's** `isError` is not this: an action that failed is reported
  inline by the feature component that owns the action, and the rule leaves it
  alone.

## Styling (external CSS — Tailwind v3 + CSS Modules)

Full rules: [`.claude/rules/tailwind-css.md`](.claude/rules/tailwind-css.md).
Use the `tailwind-component-styles` skill and the `tailwind-css-reviewer`
subagent. Enforced by the `style` harness gate and a Stop hook
(`scripts/check-styles.mjs`).

- **No Tailwind strings in TSX, ever.** `className` holds only `styles.*` refs
  (or a merged caller `className`). No inline utilities, no ternary strings, no
  style consts — a lone `mt-2` still goes in the module.
- **Styled component → sibling `Component.module.css`**, imported as `styles`.
  Unstyled components (providers, guards, thin routes) get no module.
- **v3 idiom:** no `@reference`/`@theme`/`@utility`/`@layer` in a component
  module (`@reference`/`@theme`/`@utility` are v4; `@layer` is v3 but needs a
  `@tailwind` context the module lacks). Modules use bare `@apply` at the top
  level; never name a class the same as a utility it applies. Tokens live in the
  shared preset
  (`packages/shared-ui/tailwind-preset.cjs`, mirroring the Fedi Design System);
  reference tokens (`text-ink`, `bg-surface`, `rounded-card`, `text-status-*`),
  never hardcoded hex/px.
- **Sharing ladder** (mirrors the import direction): component module → app
  `shared/styles/utilities.css` → `packages/shared-ui/styles/`. The shared
  `utilities.css` dictionaries are loaded into Tailwind via the config
  (`load-utilities.cjs`), not `@import`ed as CSS, so `@apply uX` resolves in
  isolated modules (dev + build). shared-ui
  components reference preset tokens + shared-ui styles only, never an app's CSS.
- Conditional styling → `styles.*` refs or data attributes, never interpolated
  class names.

## Alias wiring (in place)

The `@/` alias is wired for both apps:

- each app `tsconfig.json` → `baseUrl: "."` + `paths` (`"@/*": ["src/*"]`).
- each app `vite.config.ts` → `resolve.alias` (`@` → `./src`).
- Vitest reads the Vite config, so unit tests resolve `@/` automatically.

<!-- agent-toolkit:feature-folders:start -->
## Architecture: feature-based folders (ESLint-enforced)

- Shared (`packages/*`, `apps/*/src/shared`) — global code. Imports from shared only.
- Features (`apps/*/src/features/*`) — one feature per folder. Imports from shared and its own folder only. Never other features, never app.
- App (`apps/*/src/app`) — routes, pages, entry points. Glue layer; the only place importing features.
- New module: bound to one feature → that feature's folder. Global → shared. In doubt → shared.
- Two features need the same code → lift it into shared. No cross-feature imports, ever.
<!-- agent-toolkit:feature-folders:end -->

<!-- agent-toolkit:react-compiler:start -->
## React Compiler — Rules of React (compiler-enforced)

The compiler auto-memoizes components and silently skips any that break these rules
(no error, just no optimization). Keep ESLint (react-hooks recommended-latest) green.

- Render is pure: same props/state → same JSX. No side effects or mutation in render.
- Never mutate props, state, or hook return values — build a new local value instead.
- Hooks: top level, unconditional, stable order.
- No reading/writing refs during render.
- Call components as JSX (`<Foo />`), never as functions.
- Escape hatch: `"use no memo"` at the top of a component — temporary, with a TODO to fix.
- New components: compiler-clean by default. If ESLint flags one, fix it — don't disable the rule.
<!-- agent-toolkit:react-compiler:end -->

<!-- Quality harness distillation. Full rules: docs/clean-code.md (read it when creating/refactoring components, hooks, or tests). -->

## Code standards (distilled — full doc: docs/clean-code.md)

- Courier test: hook result forwarded to exactly ONE child → move the hook into that child. Exempt: useContext/useRef/router hooks, `// hoisted: waterfall`.
- Never pass setState setters, `dispatch`, or whole hook-return objects as props. Communicate via `onChange`/`onCommit`; initialize via `defaultValue`.
- Names describe domain purpose, never structure: no `Wrapper`/`Container`/`Helper`/`Utils`/`data2`. `onX` props, `handleX` handlers.
- Files: `PascalCase.tsx` components, `useCamelCase.ts` hooks, `kebab-case/` folders.
- New code goes in `apps/*/src/features/<name>/`; features never import other features. Shared by 2+ features → `apps/*/src/shared/` or `packages/*`; pages/routes glue only (flow: shared ← features ← pages/app). Full layout: `.claude/rules/folder-structure.md`.
- Rule of three before extracting shared components; name extractions for domain purpose.
- Protected dirs (see harness.config.json) are closed for modification — extend via new files implementing existing interfaces.
- Logic lives in hooks; components are JSX + wiring. Helpers and constants outside. No magic values.
- TDD: one failing test → minimal code to pass → repeat. Tests accompany code in the same commit. Tracer bullet for a new flow = its first e2e.
- Tests: SUT imported last, "should…" names, no conditionals, semantic queries, literal expected values.
- Use the code-reviewer subagent only when explicitly asked or at commit time; do not spawn subagents for tasks a direct approach handles.
- Use the project's task runner; never run destructive commands; no recap .md files unless asked.
