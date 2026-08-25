import type { GetHealthResponse } from '@operator-ui/types';
import { describe, expect, it } from 'vitest';
import { isRestoreMode, startingReason } from '@/shared/api/restoreMode';

const health = (mode: GetHealthResponse['mode']): GetHealthResponse => ({
  overall_status: 'warning',
  mode,
  observed_at: 1721476800,
  components: [{ component: 'daemon', status: 'warning', detail: null, observed_at: 1721476800 }]
});

describe('isRestoreMode', () => {
  it('should return true when the daemon reports restore mode', () => {
    expect(isRestoreMode(health('restore'))).toBe(true);
  });

  it('should return false for a normally booted daemon', () => {
    expect(isRestoreMode(health('normal'))).toBe(false);
  });

  // A live restore inside a normally booted daemon is not restore-only mode:
  // the full Admin API comes back once the swap lands, so the operator belongs
  // on the normal console, not the standalone recovery one.
  it('should return false while a normal daemon reloads through a live restore', () => {
    expect(isRestoreMode(health('reloading'))).toBe(false);
  });

  it('should return false when no runtime generation is installed', () => {
    expect(isRestoreMode(health('no_runtime'))).toBe(false);
  });

  it('should return false when health is undefined', () => {
    expect(isRestoreMode(undefined)).toBe(false);
  });
});

describe('startingReason', () => {
  // Neither is restore-only mode, and neither is an unreachable daemon. Before
  // this the boot gate had no third answer, so both fell through to the
  // daemon-unreachable screen.
  it('should name a live restore', () => {
    expect(startingReason(health('reloading'))).toBe('reloading');
  });

  it('should name a daemon that has not built its first runtime', () => {
    expect(startingReason(health('no_runtime'))).toBe('no-runtime');
  });

  it('should report nothing for a serving daemon', () => {
    expect(startingReason(health('normal'))).toBeNull();
  });

  // Restore-only boot has its own console; this must not steal it.
  it('should report nothing for restore-only mode', () => {
    expect(startingReason(health('restore'))).toBeNull();
  });

  it('should report nothing when health is undefined', () => {
    expect(startingReason(undefined)).toBeNull();
  });
});
