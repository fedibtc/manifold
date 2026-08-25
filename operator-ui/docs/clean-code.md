<!-- GENERATED from standards/ by scripts/generate-standards-doc.mjs.
     Do not edit. Edit the shard, then re-run. CI runs this with --check. -->

# Clean Code Standards

Aggregate view. Canonical source is `standards/` (also published at `harness/standards/`); each rule ID below
resolves to exactly one shard. Class suffixes: `D` deterministic, `G` guidance,
`C` contextual, `A` approval.

## Naming and files

<sub>source: `standards/common/naming-and-files.md`</sub>

- `NAME-001 M` — Components are nouns named for domain purpose, not structure.
- `NAME-002 M` — Functions are verbs; booleans are predicates.
- `NAME-003 D` — Components use `PascalCase.tsx`; hooks use
  `useCamelCase.ts`; folders use kebab-case; other TS files use camelCase.
- `NAME-004 M` — Avoid vague structural names (`Wrapper`, `Container`,
  `Helper`, `Manager`, `Utils`, `data`, `temp`) and numeric suffixes.
- `NAME-005 D` — Callback props use `onX`; local handlers use `handleX` where
  the project profile configures that convention.
- `MODULE-001 D` — At most one exported React component or hook per file.
- `MODULE-002 P` — Helpers and constants may colocate when they serve the
  file's one cohesive concern.

Project profile owns exact placement, barrel-export, and test-colocation rules.
Never infer these from a different client project.

## Architecture and abstraction

<sub>source: `standards/common/architecture-and-abstraction.md`</sub>

## Placement and dependency direction

- `ARCH-001 D` — Follow configured shared ← feature ← composition/app
  dependency direction.
- `ARCH-002 M` — New domain code starts feature-local.
- `ARCH-003 C` — Move code to shared only for real cross-domain use or a proven
  stable cross-cutting seam.
- `ARCH-004 D` — App/page layer owns routing and composition, not business logic.
- `ARCH-005 M` — Each module owns one cohesive concern.
- `ARCH-006 M` — Prefer deep interfaces hiding complexity over pass-through
  modules.
- `ARCH-007 C` — Add a strategy/implementation only when a real varying seam
  exists; do not manufacture one for a single variant.
- `ARCH-008 D` — Protected paths obey configured write policy.

## Reuse and duplication

- `ABS-001 C` — Default to rule of three before shared extraction.
- `ABS-002 C` — Two occurrences may earn extraction when evidence shows the
  same domain or infrastructure concept.
- `ABS-003 M` — Duplication means “extract or justify,” never automatic extraction.
- `ABS-004 M` — Never create an abstraction only to satisfy a size/count metric.
- `ABS-005 P` — Repeated JSX layout may become a domain-named component using
  composition for varying content.
- `ABS-006 P` — Keep intentional contextual duplication local and record why
  when reported.

## Components and state

<sub>source: `standards/react/components-and-state.md`</sub>

- `REACT-001 M` — Components describe UI; cohesive stateful/business
  orchestration lives in the correct hook or service boundary.
- `REACT-002 M` — State and hooks live in the lowest component consuming them.
- `REACT-003 M` — If a parent only forwards one hook result to one child, the
  child calls the hook.
- `REACT-004 M` — Legitimate hoists: parent consumption, multiple consumers,
  cross-field live validation, atomic actions/workflows, or documented
  request-waterfall prevention.
- `REACT-005 M` — Never pass setters, dispatch, or whole hook-return objects.
  Pass narrow values/default values and semantic `onX` callbacks.
- `REACT-006 D` — Hooks run unconditionally at top level; render stays pure;
  configured React Compiler lint passes.
- `REACT-007 P` — Keep JSX template-like using named derived values and handlers.
- `REACT-008 P` — Memoize only with cost or stability evidence.
- `REACT-011 P` — Two or more sibling JSX regions with disjoint reactive-state
  and handler clusters trigger an observation, not automatic extraction.
- `REACT-012 M` — A meaningful child owns state used only by its subtree.
  Shared validation, atomic save/reset/undo/submit, and coordinated workflows
  stay at the nearest common owner.
- `REACT-013 D` — Component definitions have stable module-scope identity.
- `REACT-014 M` — Extracted regions use domain names and expose a smaller
  interface than the behavior hidden.
- `REACT-015 C` — Move behavior tests to an extracted child's interface; keep
  parent coverage for composition and cross-child behavior.
- `REACT-016 M` — A feature component calls the domain hook it directly consumes.
- `REACT-017 C` — Multiple children may call narrow hooks backed by the same
  query cache/store only when multi-subscriber behavior is safe, requests
  deduplicate, and duplicate imperative effects cannot occur.
- `REACT-018 M` — Repeated local-state-hook calls create independent state.
  Shared local state stays at the nearest common component/orchestration hook.
- `REACT-019 M` — Do not add custom React Context solely for local page,
  feature, form, or workflow state. Infrastructure providers remain allowed.
- `REACT-020 M` — Generic shared presentation remains prop-driven. A
  feature-connected component may adapt domain-hook output to that interface.

Raw hook count and raw child count never prove a violation.

## Hooks

<sub>source: `standards/react/hooks.md`</sub>

- `HOOK-001 M` — A hook owns one cohesive flow or reason to change.
- `HOOK-002 M` — Extract a meaningful stateful/reusable seam, not a long file.
- `HOOK-003 M` — Return an object with named fields, never a tuple.
- `HOOK-004 P` — Use an options object for more than two arguments.
- `HOOK-005 M` — Keep pure helpers outside; pass dependencies explicitly.
- `HOOK-006 M` — Expose caller-needed view decisions and derived values without
  leaking unrelated collaborators.
- `HOOK-007 C` — Test a hook directly when it is an independent seam; owning-
  component coverage may suffice otherwise.
- `HOOK-008 P` — More than three non-exempt coordinating hooks trigger an
  `hook-composition-signal` observation, not mandatory extraction.
- `HOOK-009 C` — Use `use<ComponentName>` only when calls form one flow and the
  hook creates a smaller caller interface. Evidence-backed `keep-local` is valid.
- `HOOK-010 M` — Orchestration hooks return meaningful domain slices, view
  state, and actions; never raw query/mutation objects or `{ apis }`.
- `HOOK-011 M` — Nested hooks group one domain/user capability, never calls
  chosen only to reduce count.

Count domain/data hooks, `useState`, `useReducer`, and `useEffect` for the
composition signal. Exclude `useRef`, `useContext`, router/params hooks,
`useMemo`, and `useCallback`. An observation never autofixes.

## Styling and async UI

<sub>source: `standards/react/styling-and-async-ui.md`</sub>

## Async UI

- `REACT-009 M` — Model loading, error, empty, and populated states when applicable.
- `REACT-010 C` — Use optimistic updates only when reversibility, conflict risk,
  and product expectation support them.

## Styling

- `STYLE-001 M` — Project design source and system override generic design skills.
- `STYLE-002 M` — Search existing components, tokens, and utilities first.
- `STYLE-003 C` — Promote repeated style only at nearest valid shared scope.
  Tool: `scripts/check-css-dupes.mjs` / `pnpm lint:css-dupes` emits
  `local/css-apply-dupe-signal` observations. Never auto-extract; decide
  promote / keep-local / coincidental-match with evidence. Pilot threshold
  target: ≥3 identical `@apply` values after excluding known utilities.
- `STYLE-004 D` — Machine-check project-forbidden styling forms.
- `STYLE-005 M` — Visual states remain accessible and do not rely on color alone.

For the `operator-ui` profile:

- `OUI-STYLE-001 D` — No Tailwind utility strings in TSX.
- `OUI-STYLE-002 D` — Styled component owns sibling CSS Module; unstyled glue
  receives no empty module.
- `OUI-STYLE-003 D` — Tailwind v3 modules use bare `@apply`; no v4 directives
  or `@layer` inside component modules.
- `OUI-STYLE-004 M` — Reuse Fedi tokens/components before hardcoded values or
  another UI kit.
- `OUI-STYLE-005 M` — Conditional styling uses module references/data
  attributes, not interpolated utility strings.

## Testing

<sub>source: `standards/common/testing.md`</sub>

## Behavior and quality

- `TEST-001 D` — Test names use the project behavior form, currently
  `it("should …")`.
- `TEST-002 M` — Assert observable behavior through public interfaces.
- `TEST-003 D` — Ordinary test bodies remain branch-free; use separate tests
  or parameterization.
- `TEST-004 P` — Tests read straight Arrange → Act → Assert.
- `TEST-005 D` — UI tests prefer semantic queries; test IDs need a documented
  absence of an accessible alternative.
- `TEST-006 M` — Expected values use independent literals/spec examples, not
  implementation-equivalent recomputation.
- `TEST-007 M` — Restore overridden globals/timers; prevent leaked state.
- `TEST-008 M` — Bug fixes include a regression test failing for the original reason.
- `TEST-009 M` — Changed behavior and tests ship in the same change set.
- `TEST-010 C` — Page tests cover composition/states; feature tests cover
  detailed behavior.

## Reuse

- `TEST-REUSE-001 M` — Search render wrappers, fixtures, factories, mocks, and
  helpers before creating setup.
- `TEST-REUSE-002 M` — Parameterize tests differing only by inputs and expected values.
- `TEST-REUSE-003 C` — Extract stable repeated test infrastructure at nearest
  scope: local → feature → shared test support.
- `TEST-REUSE-004 M` — Keep SUT/scenario-specific setup local unless the same
  concept repeats.
- `TEST-REUSE-005 M` — Do not create one-use helpers.
- `TEST-REUSE-006 P` — Helpers use parameter objects and purpose-revealing names.
- `TEST-REUSE-007 M` — Helpers must not hide scenario intent behind generic flow.
- `TEST-REUSE-008 C` — Mock factories branch only when simpler parameterized
  fixtures cannot represent the scenario clearly.

## Placement and E2E

- `TEST-PLACE-001 D` — Test placement follows the project profile.
- `TEST-E2E-001 M` — E2E covers critical journeys, not behavior cheaper below UI.
- `TEST-E2E-002 D` — No fixed sleeps; use web-first assertions.
- `TEST-E2E-003 C` — Start a critical new flow with one tracer-bullet E2E when
  required by profile.

Mocks load before the SUT. SUT import remains last where the project test runner
depends on module-hoisting order. Move extracted-child behavior assertions to
the child interface; keep parent tests for composition and cross-child behavior.

## Accessibility and security

<sub>source: `standards/common/accessibility-and-security.md`</sub>

## Accessibility

- `A11Y-001 D` — Configured static accessibility lint passes.
- `A11Y-002 M` — Controls use correct semantic elements and accessible names.
- `A11Y-003 M` — Labels associate with inputs.
- `A11Y-004 M` — Keyboard and focus behavior support the interaction.
- `A11Y-005 M` — Dynamic loading, error, and result changes are communicated.
- `A11Y-006 A` — Conformance-target changes require project decision.

## Security and specifications

- `SEC-001 D` — Secret scanning and protected-path checks block violations.
- `SEC-002 A` — Credential, network-facing, local-service, and security-policy
  changes follow project approval.
- `SPEC-001 M` — Code and declared project specifications remain synchronized.
- `SPEC-002 A` — Implementation-driven specification changes require approval.

## Scope and enforcement

<sub>source: `standards/common/scope-and-enforcement.md`</sub>

## Mandatory scope

- `SCOPE-001 M` — Every changed line traces to requested work.
- `SCOPE-002 M` — Do not refactor or reformat unrelated code.
- `SCOPE-003 M` — Remove only orphaned code created by the change unless asked.
- `SCOPE-004 D` — Never execute configured destructive or forbidden operations.
- `SCOPE-005 A` — Commit, push, publish, dependency additions, and public API
  breaks require authority unless the workflow explicitly grants it.
- `VERIFY-001 D` — Never complete with a failed, unavailable, or misconfigured
  mandatory gate.
- `VERIFY-002 M` — Correctness claims cite command, test, or browser evidence.

## Finding routes

- Exact mechanical violation → `block`.
- Safe bounded correction → `repair`, once.
- Contextual choice → `decision-required`.
- Useful concern or opportunity → non-blocking `observation`.

Observations never autofix. One finding fingerprint receives at most one
automatic repair. A repeated fingerprint becomes a decision or observation,
never another automatic refactor. A valid `keep-local` decision passes.
Deterministic gates cannot be waived by a contextual decision.

## Human approval

Request approval for materially ambiguous acceptance criteria, missing
architecture decisions, public API/cross-feature ownership changes, dependency
changes, design-system deviations, security-sensitive changes, specification
changes, and external writes not already authorized.

