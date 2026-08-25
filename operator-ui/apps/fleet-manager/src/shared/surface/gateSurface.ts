export type GateOwner = 'boot' | 'setup';

export type GateSurface = 'boot' | 'auth' | 'daemon-error' | 'setup';

/** BootGate outranks SetupGate: it sits above it, so whatever it is showing is
 *  what is on the screen. */
const PRIORITY: readonly GateOwner[] = ['boot', 'setup'];

const owned = new Map<GateOwner, GateSurface>();
const listeners = new Set<() => void>();

const announce = () => {
  for (const listener of listeners) listener();
};

/**
 * Which surface a gate is rendering, keyed by the gate that owns it.
 *
 * Four surfaces have no route of their own — the boot screen, the sign-in prompt,
 * the daemon-error screen and the setup wizard — so a pathname cannot name them.
 * Each gate declares and retracts only its own value, which is what stops a
 * parent's cleanup from clearing a child's.
 *
 * Lives in `shared`, not in `mocks`: `BootGate` and `SetupGate` are production
 * components and must not import `@/mocks/*`, or the mock world reaches the
 * production bundle.
 */
export const gateSurface = {
  set(owner: GateOwner, surface: GateSurface): void {
    if (owned.get(owner) === surface) return;
    owned.set(owner, surface);
    announce();
  },

  clear(owner: GateOwner): void {
    if (!owned.delete(owner)) return;
    announce();
  },

  /** A string or `null`, both stable by value, so `useSyncExternalStore` is safe. */
  getSnapshot(): GateSurface | null {
    for (const owner of PRIORITY) {
      const surface = owned.get(owner);
      if (surface) return surface;
    }
    return null;
  },

  subscribe(listener: () => void): () => void {
    listeners.add(listener);
    return () => {
      listeners.delete(listener);
    };
  }
};
