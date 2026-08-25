export type RouteKey = string;

export interface ScenarioNote {
  /** What this scenario puts the mock world into. */
  desc: string;
  /** Which routes change when it is loaded — drives the per-page panel tab. */
  affects: RouteKey[];
}

export interface ScenarioCatalogEntry extends ScenarioNote {
  name: string;
}

export interface StorageAdapter {
  load(key: string): string | null;
  save(key: string, value: string): void;
}

/** One knob on the Global tab. Apps describe their own world fields this way so
 *  the panel never learns that FMan has an `authMode` or FLIP a `bootMode`. */
export interface MockControl {
  id: string;
  label: string;
  kind: 'number' | 'select';
  /** Required for `kind: 'select'`, ignored otherwise. */
  options?: readonly string[];
  read(): string;
  write(value: string): void;
}

export interface ErrorInjection {
  /** Every verb the app dispatches — read off its verb map, so this list cannot
   *  drift from what is actually routed. */
  verbs: readonly string[];
  /** Selectable error values: `ServiceErrorCode`s for FLIP, canned messages for
   *  FMan, whose forced errors are message strings. */
  codes: readonly string[];
  /** Must return a fresh object. The world's `forcedErrors` is mutated in
   *  place, so handing it back directly gives every reader an identity that
   *  never changes and a panel that never notices an injection. */
  active(): Readonly<Record<string, string>>;
  set(verb: string, code: string | null): void;
}

/** Everything the panel needs beyond the scenario catalog, supplied per app.
 *
 *  Every writer here — `controls[].write`, `errors.set`, `patch` — owns
 *  persisting the world and notifying the store. The panel does not do it on
 *  their behalf, because the same writes arrive from `window.__mockControl`
 *  with no panel in the loop and must behave identically. */
export interface PanelConfig {
  controls: readonly MockControl[];
  errors: ErrorInjection;
  /** Dotted path into the world plus an already-parsed value — the escape hatch
   *  for state the typed controls do not cover. */
  patch(path: string, value: unknown): void;
}

/** Which verbs MSW has served, stamped with the route key that was showing at
 *  the time. Lets the per-page tab list what a page actually calls without a
 *  hand-written route→verbs map that silently rots. */
export interface VerbLog {
  record(verb: string): void;
  /** Stable reference while unchanged, as `useSyncExternalStore` requires. */
  list(routeKey: string): readonly string[];
  clear(routeKey: string): void;
  subscribe(listener: () => void): () => void;
}

/** Everything the store needs to know about an app's world, so the store itself
 *  stays generic over FMan and FLIP. */
export interface WorldSource<W> {
  /** Suffix of the localStorage key: 'fman' | 'flip'. */
  appKey: string;
  defaultScenario: string;
  has(name: string): boolean;
  build(name: string): W;
  /** Copy state that must survive a scenario switch from the outgoing world into
   *  the freshly built one. For dev-session artifacts — an authenticated session,
   *  a bootstrap token — that describe the mock rather than the scenario. */
  carryOver?(previous: W, next: W): void;
}

export interface ScenarioStore<W> {
  getWorld(): W;
  getScenario(): string;
  /** The scenario `reset()` returns to. Lets a control surface tell an
   *  overridden world from an untouched one. */
  getDefaultScenario(): string;
  setScenario(name: string): void;
  reset(): void;
  /** Write the current world back to storage. Called after mutating verbs, and
   *  deliberately silent: notifying here would refetch every active query on
   *  each mutation. */
  persist(): void;
  /** Announce that the world changed. Control surfaces call it after
   *  `persist()`; verbs do not. The two are separate so that a panel knob
   *  refreshes the screen while a mutating verb keeps today's behaviour. Also
   *  marks (and persists) the world as dirty — by the writer contract a
   *  notification can only be a hand-made override. */
  notify(): void;
  /** True once a control surface has written since the last scenario load —
   *  the world carries overrides its scenario name alone will not reveal. */
  isDirty(): boolean;
  /** The persisted blob plus the app key, pretty-printed: a copyable recipe of
   *  the effective debug state for bug reports and handoffs. */
  exportState(): string;
  /** Bumped by every notification. Backs `useMockRevision`, because a control
   *  change leaves `getScenario()` — the scenario snapshot — untouched. */
  getRevision(): number;
  subscribe(listener: () => void): () => void;
}
