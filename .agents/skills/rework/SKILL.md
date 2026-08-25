---
name: rework
description: Interactive session that re-derives one system from its requirements by running a battery of passes (assumptions, ownership, invariants, encapsulation, mechanisms) to a fixed point; commits are the conversation. Use when the user asks to rework code, simplify a component, question a design's assumptions, or review for anything other than bugs.
---

# Rework

An interactive working session with the author(s), not a review. Of the two
review disciplines — correctness (bugs) and everything else — this is
everything else: assumptions, structure, simplification, invariants. Think
about the final global complexity of the system, not the current code. Big
changes are fine. Prefer re-engineering over patching.

## A session is passes run to a fixed point

A session targets one system (e.g. "the fleet manager" — its crates and
spec chain). Organize all work within it as **passes**. A pass is one lens
over the whole system: one question, the signals that answer it, and a clean
condition ("this pass found nothing against the current code").

Passes exist because reading code only answers the question you brought to
it: reading for structure does not surface unpromoted invariants, hunting
invariants does not surface encapsulation holes. A lens you never run is a
set of findings you only get when the author prompts you with it — and
every such prompt is a session failure with a precise diagnosis: a missing
or unrun pass. Fix it twice: run the pass now, and add or sharpen the pass
in this skill.

Scheduling: global passes first, because their changes invalidate local
work. Then local passes, module by module — finish a pass through a module
before switching lenses; depth beats coverage *within* a pass. When local
passes have changed the code, re-run the global passes over the new,
smaller system (a contained module may reveal a mergeable boundary; a
deleted mechanism may orphan a layer), then go local again. **Done is a
fixed point, not a feeling**: every pass in the battery runs clean against
the current code. The pass structure is for coverage accounting — which
lenses ran, which are clean — not a form to fill; a shallow sweep of all
passes is worth less than three passes run to completion.

### Global passes (whole-system)

1. **Assumption pass** — build the requirement→code trace; question layer
   boundaries, verbs/contracts, and each requirement's authority (see
   *Question the choices*). Check imports only point down the layer stack.
   This is where assumption *changes* happen — the big deletions and
   re-engineering. Run it first; don't polish anything local until it's
   done.
2. **Ownership pass** — one owner per concept; every contract two programs
   must agree on (wire shapes, constants, log formats one side prints and
   the other parses) has exactly one definition. Signals: the same literal
   or struct in two crates; a parser in one program for a printer in
   another; a constant restated in prose docs; "must match X" comments.
3. **Module-topology pass** — the map of modules must match the map of
   concepts: one module per concept or protocol role, layered so imports
   only point down. Verify any claimed layering against the real import
   graph — an architecture doc's module order is a claim, not a fact.
   Signals:
   - a module whose name disclaims ownership ("provisional", "misc",
     "util") or whose doc says its contents belong elsewhere — either
     they found their home long ago or the wished-for home now exists;
   - one file serving two protocol roles or trust positions (payer and
     payee, client and server) — split by role, not by size;
   - vocabulary defined above the layer that persists or transports it
     (storage importing domain types from the runtime above) — move the
     shapes below both consumers;
   - a module exporting one function with one caller: fold it into the
     owner of the type it operates on — an invariant checked next to
     one side of a protocol often belongs on the shared type where the
     other side can check it too;
   - duplicate definitions of a concept across sibling crates when a
     shared home already exists and is even partially in use.

### Local passes (module by module, each with one cross-module sweep)

"Local" scopes the *investigation*, not the changes: a pass starts from
one module for coverage accounting, but a finding's fix lands wherever
the finding lives — wire crates, sibling layers, specs. Deferring a
global fix because the pass is "local" is the failure mode, not the
discipline; if two findings share one global surface, batch them into
one reshaping now rather than leaving both half-fixed.

4. **Invariant pass** — enumerate every invariant held below its affordable
   rung and promote it (see *the enforcement ladder*). Signals:
   - an `expect`/`panic` justified by prose ("the allocator never…") — the
     prose is an invariant nobody promoted;
   - a doc comment promising a shape ("None iff Reserved") that the type
     admits violating;
   - a wire/API field that only ever takes one value;
   - an `Option` filled by one phase and unwrapped by another;
   - a validation performed at one call site instead of in a constructor;
   - a domain value travelling as `String`/`Vec<u8>` between its creation
     and its wire/display edge (hex keys, ids, addresses): two concepts
     sharing `String` lets every mix-up compile, and each hop re-parses
     or trusts blindly. Parse into the domain type at the boundary,
     stringify only at the edge. Signal within the signal: a test
     round-tripping a nonsense literal (`"cafe"`) through a field that is
     supposedly a key — the type admits values the domain never will.
5. **Encapsulation pass** — for each module: can code *outside* it violate
   the discipline it maintains? Different question from the invariant pass
   (that one asks what property is held informally; this one asks who can
   break the owner's discipline from outside). Signals:
   - `pub` fields on a type whose module maintains a discipline over them
     (guarded constructor, set-once, one-way mirror) — the discipline can
     be walked around;
   - a type with a validating constructor that is also constructible
     field-by-field;
   - a "callers must / never" doc comment — the contract sits on the wrong
     side of the boundary;
   - tests building by literal what production builds via constructor (the
     boundary has holes and the tests found them first);
   - a contract not much smaller than its implementation — nothing was
     encapsulated, you just added a folder.
6. **Mechanism pass** — every lock, cache, retry, flag, and knob names
   exactly what it protects. A mechanism doing two jobs hides the
   assumption that one granularity fits both — split by responsibility.
   Defensive code is a signal: it marks an invariant nobody promoted;
   promote it and delete the defence, or discover the invariant is false.
   Never just leave it.
7. **Interruption pass** — for every multi-step sequence, enumerate what
   can halt execution partway (process crash, task cancellation, shutdown,
   client disconnect) and check each halt has a recovery story. Crash-safe
   is not cancellation-safe: a crash gets a rebuild-from-durable-state
   restart, a cancelled task leaves a live process with half-done work and
   stale in-memory state that nothing rebuilds. Signals:
   - an await between a durable write and the in-memory update that
     mirrors it;
   - a "transiently None/invalid" comment — what if execution stops while
     it's transient?;
   - sequences engineered for crash recovery whose no-cancellation
     assumption lives in another crate (who spawns the handler, detached
     or aborted?) — if the analysis is sound only because of a convention
     elsewhere, record the convention at both ends with its dependents
     named.
8. **Test-quality pass** — for each property the code's comments argue,
   would a test fail if it broke? Signals: an ordering or crash-window
   argument in a comment with no test forcing that failure path; a
   mechanism built for failures whose tests only exercise success; a
   fake/mock that cannot fail, making failure-path tests impossible to
   write (extend the fake first).
9. **Observability pass** — for each failure class, can an operator
   diagnose it from logs alone? Signals: a deliberately coarse wire error
   whose detailed log twin drops the cause chain (`Display` on a wrapping
   error prints only the top message); the boundary where error detail
   dies having no log at all.
10. **Decomposition pass** — every private function boundary must earn its
   fold: it names a concept and returns a *whole* value. A single caller
   is the prompting signal, not the verdict — the verdict is whether the
   boundary carries a contract smaller than its body. Signals:
   - a return value carrying an undischarged obligation the caller must
     remember (a spawned child whose pipes still need pumps attached, a
     guard needing a follow-up write): move the obligation inside the
     function or inline it — the intermediate state should never be
     holdable;
   - a helper extracted only for length, whose signature restates the
     caller's locals (long parameter list in, tuple out): inline it;
     with one user and no contract, the boundary is a fold in the
     paper, and every future edit pays it;
   - staged construction across private helpers where an intermediate
     stage violates the module's own rules — collapse the stages.
   Counterweight: a single-caller function that owns a real discipline
   (a spawn contract, a parser, an unsafe block's justification) is
   contained complexity — keep it, and check instead that its return
   type is valid without caller follow-up. Watch for boundaries that
   *were* whole and got demoted by a later change (the return type grew
   an obligation nobody re-examined).

### Working rules

- **Checkpoint before switching passes or modules**: commit the coherent
  change and land its doc repairs first. Then anything the context later
  loses (long sessions get compacted) is reconstructible from the repo —
  commits and repaired specs are the durable state, context is cache.
- A pass is a lens, not a fence: follow a finding wherever it leads (wire
  types, sibling layers, specs), then return to the pass.
- No target system given? Start with a cheap scout: rank where deletable
  complexity concentrates and which specs are stale, pick the target with
  the author, and go deep — never shallow-touch everything.
- Doc repair (below) is what lets passes compound: the assumption pass's
  recorded trace is every later pass's map, and this session's repaired
  specs are the next session's cheap startup. If the target's docs are
  stale, repairing them is a first-class early move, not overhead.

## Commits are the conversation

- Write code freely; code is cheap to throw away. A commit is the
  highest-bandwidth question that exists: "is this what you meant?" asked in
  executable form. Produce it, show it, let the author react.
- Commit after each coherent change (one assumption change per commit), with
  a message that names the assumption changed and what the change deleted.
- The deliverable is the commit stack + the open decision points that didn't
  resolve in-session + the doc repairs. Not a findings document.
- A session that changes zero code but promotes an invariant, pins a test,
  and repairs three spec passages is a success.

## Question the choices that led here

The assumption pass in detail:

- For each cluster of complexity, name the requirement that forces it, then
  check that requirement's **authority**: reasoned constraint, canonical
  decision, provisional addition, or habit. Complexity serving an unowned
  requirement is deletable. Read the docs/spec chain, not just the code —
  the biggest wins usually hide there.
- Docs are fragile *asymmetrically*: trust constraints that come with
  reasons; treat bare decisions and anything marked provisional/pending as
  question candidates.
- When a stated doctrine forces contortions, find its precise weaker form
  instead of abandoning it (e.g. "never persist derived state" → "never
  persist anything that can go stale"; one-way facts are fine).

## Do vs. ask

People are bad at specifying and excellent at reacting, so bias toward
concrete moves — classify what a change deletes:

- **Mechanism deletion** (same observable behavior, less machinery): just do
  it and show the commit. If you catch yourself presenting a list of
  "worth doing, awaiting go-ahead" items that are mechanism-level, that is
  a bug — do them; the commit is the question.
- **Intent-preserving reshaping** (same underlying want, different
  observable shape — wire fields, verbs, UX affordances): **do + tag**.
  Make the change and leave an inline `@owner` comment on the diff naming
  what was removed/reshaped and why, plus a note in the doc chain — the
  owner reviews a concrete diff instead of adjudicating a hypothetical.
  Reserve the veto-able written proposal ("I'd change X to Y; you lose A,
  gain B") for changes that need authority the session doesn't have (a
  canonical external doc, another team's contract).
- **Scope reduction** (a want actually disappears): label it as such and get
  sign-off. The lazy failure mode is disguising scope reduction as
  simplification. Test: can the user still do everything they actually
  cared about, possibly spelled differently?

A "don't do X yet" from the author freezes X, not the session — keep making
the moves that don't depend on it. Asking still beats removing code on a
whim, but every question you ask that comes back "just do it" is a
calibration error to correct in-session.

## Invariants: enforce at the highest affordable rung

The invariant pass's ladder — each rung down moves the failure later and
from a machine to a human:

unrepresentable by types (compile time)
> valid by construction (creation site)
> runtime mechanism that fails loudly — set-once, uniqueness, exclusive
  locks (first violating use)
> a test pinning the property (CI)
> documented convention (review; say why it can't go higher).

Above every rung sits **dissolution**: make the invariant unstateable
instead of enforcing it — two maps that must agree become one map holding
the thing; two constants that must match become one definition. When an
invariant relates two copies, first ask whether the second copy needs to
exist.

Judging "affordable": the promotion must cross a boundary someone can
actually violate — enforcing a discipline already sealed behind one
module's private state churns call sites for nothing (record the
rejection). It's finished only when the defence it replaces — guard,
`expect`, prose — is deleted in the same commit. Cross-boundary invariants
(two crates, code + transport) often cap at documentation: then document
both ends, each naming the other's dependents.

What types must **not** assert: nothing time can falsify (typestate over an
external process is a lie by use time — TOCTOU; what an observation
*returned* is safe, a fact about the past), and no collapsing genuine
crash-window partiality into a prettier progression enum.

The goal is that local discipline becomes a global property because a
machine checks it, not a reviewer. Trace bugs upward: enforce the class
away, not the instance. Sometimes the honest outcome is that the invariant
is *false* — a bug find, the pass's other deliverable.

## Contained complexity

The encapsulation pass's standard: judge complexity at the interface, not
the implementation. Essential complexity has to live somewhere; give it one
home with a narrow contract so its cost is paid once, not ambiently by every
caller. Buying global simplicity with contained local complexity is a good
trade. Encapsulation is only real if the boundary is enforced (private
fields, validating constructors, typed states) — otherwise it is a
convention, and conventions are just docs.

## Leave things better recorded

- Structural changes ship with their property pinned as a test, so the
  reason for the structure survives the next refactor.
- Doc repair is part of the session, not a courtesy: every answered
  question, promoted invariant, and confirmed provisional decision lands in
  the project's doc/spec chain in the same commit series. A session that
  extracts intent and doesn't record it has strip-mined the author.
- Rank moves by what they delete: an assumption change that removes a knob,
  a loop, and a lock-hold beats ten local rewrites. Stop when the author
  says so, or at the fixed point — and "fixed point" is a claim that needs
  evidence: every pass in the battery, global and local, has run clean
  against the current code. If an author prompt ("anything type-level?"
  "enough encapsulation?") unearths real work, a pass was missing or
  unrun — run it, then add it here so the next session has the lens.
- The negative results are deliverables too: record what you examined and
  rejected with the reason (a typestate that time would falsify, an enum
  that would misrepresent crash-window partiality), so the next session
  doesn't re-litigate it.
