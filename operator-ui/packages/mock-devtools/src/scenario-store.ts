import { localStorageAdapter } from './storage';
import type { ScenarioStore, StorageAdapter, WorldSource } from './types';

/** Bump when the shape of any app's MockState changes, so a colleague's stale
 *  blob is discarded instead of half-loading. v2 added the `dirty` flag; v3 added
 *  the fleet-manager restore and onboarding transport controls, which are required
 *  fields a v2 blob cannot supply; v4 added `fman_version` to the fleet-manager
 *  onboarding response, which a v3 blob likewise cannot supply; v5 replaced the
 *  onboarding `nostr` union — a v4 blob carries `waiting_for_authorization`,
 *  which is no longer a state the daemon can report; v6 added
 *  `lifetime_remitted_msat` to the fleet-manager fee ledger, which a v5 blob
 *  cannot supply and whose absence is the omission this typing closed. */
export const STORE_VERSION = 6;

interface Persisted<W> {
  v: number;
  scenario: string;
  world: W;
  /** True once a control surface has written since the last scenario load. */
  dirty: boolean;
}

export const storeKey = (appKey: string): string => `operator-ui:dev:mocks:${appKey}`;

export const createScenarioStore = <W>(
  source: WorldSource<W>,
  storage: StorageAdapter = localStorageAdapter
): ScenarioStore<W> => {
  const key = storeKey(source.appKey);
  const listeners = new Set<() => void>();

  // The persisted world is not deep-validated. The version stamp plus a known
  // scenario name is the whole guard: a schema library for dev-only mock state
  // is not worth it when Reset is one click away.
  const restore = (): Persisted<W> | null => {
    const raw = storage.load(key);
    if (!raw) return null;

    let parsed: Persisted<W> & { seed?: string };
    try {
      parsed = JSON.parse(raw) as Persisted<W> & { seed?: string };
    } catch {
      return null;
    }

    // A test seeded a scenario name before the app booted; build it fresh.
    if (typeof parsed.seed === 'string') {
      return source.has(parsed.seed)
        ? {
            v: STORE_VERSION,
            scenario: parsed.seed,
            world: source.build(parsed.seed),
            dirty: false
          }
        : null;
    }

    if (parsed.v !== STORE_VERSION) return null;
    if (!source.has(parsed.scenario)) return null;
    return { ...parsed, dirty: parsed.dirty === true };
  };

  const restored = restore();
  let scenario = restored?.scenario ?? source.defaultScenario;
  let world = restored ? restored.world : source.build(scenario);
  let dirty = restored?.dirty ?? false;

  let revision = 0;

  const write = () => {
    storage.save(key, JSON.stringify({ v: STORE_VERSION, scenario, world, dirty }));
  };

  const emit = () => {
    revision += 1;
    for (const listener of listeners) listener();
  };

  const load = (name: string) => {
    const previous = world;
    scenario = name;
    world = source.build(name);
    source.carryOver?.(previous, world);
    dirty = false;
    write();
    emit();
  };

  if (!restored) write();

  return {
    getWorld: () => world,
    getScenario: () => scenario,
    getDefaultScenario: () => source.defaultScenario,
    setScenario: (name) => {
      if (!source.has(name)) throw new Error(`unknown scenario: ${name}`);
      load(name);
    },
    reset: () => load(source.defaultScenario),
    persist: write,
    // A notification is, by the writer contract, a control-surface write —
    // mutating verbs only persist. That makes this the one place the world
    // provably moved off its scenario by hand, so it owns the dirty flag.
    notify: () => {
      if (!dirty) {
        dirty = true;
        write();
      }
      emit();
    },
    isDirty: () => dirty,
    exportState: () =>
      JSON.stringify({ app: source.appKey, v: STORE_VERSION, scenario, world, dirty }, null, 2),
    getRevision: () => revision,
    subscribe: (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    }
  };
};
