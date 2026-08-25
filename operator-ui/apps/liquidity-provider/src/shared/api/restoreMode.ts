import type { GetHealthResponse } from '@operator-ui/types';

// Detects whether the connected daemon booted in restore-only mode.
//
// The daemon reports this as a typed `mode` on the health payload. It used to be
// a `restore_mode=true` substring inside the daemon component's `detail`, which
// the unauthenticated GET /health no longer carries: that route now withholds
// every detail string, so a marker match there would have silently stopped
// matching. `mode` crosses the unauthenticated boundary by design.
const RESTORE: GetHealthResponse['mode'] = 'restore';

export const isRestoreMode = (health: GetHealthResponse | undefined | null): boolean =>
  health?.mode === RESTORE;

// The daemon is answering but has no runtime to serve the Admin API from.
//
// `reloading` is a live restore swapping the data dir; `no_runtime` is a daemon
// that has not finished building its first generation — the Admin API binds
// concurrently with that build, so every start passes through it. Neither is
// restore-only mode and neither is an unreachable daemon, and the dashboard
// used to report both as the latter.
export const startingReason = (
  health: GetHealthResponse | undefined | null
): 'reloading' | 'no-runtime' | null => {
  if (health?.mode === 'reloading') return 'reloading';
  if (health?.mode === 'no_runtime') return 'no-runtime';
  return null;
};
