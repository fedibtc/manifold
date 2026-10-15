import { vi } from 'vitest';
import { reloadIfBundleIsStale, STALE_BUNDLE_RELOAD_KEY } from '@/features/boot/utils/staleBundle';

const makeStorage = (entries: Record<string, string> = {}) => {
  const values = new Map(Object.entries(entries));
  return {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => {
      values.set(key, value);
    }
  };
};

it('should reload once and remember the daemon version when the bundle is from another release', () => {
  const storage = makeStorage();
  const reload = vi.fn();

  reloadIfBundleIsStale('0.1.3', '0.1.4', storage, reload);

  expect(reload).toHaveBeenCalledTimes(1);
  expect(storage.getItem(STALE_BUNDLE_RELOAD_KEY)).toBe('0.1.4');
});

it('should not reload when the bundle matches the daemon', () => {
  const reload = vi.fn();

  reloadIfBundleIsStale('0.1.4', '0.1.4', makeStorage(), reload);

  expect(reload).not.toHaveBeenCalled();
});

it('should not reload again for a daemon version it already reloaded for', () => {
  const reload = vi.fn();
  const storage = makeStorage({ [STALE_BUNDLE_RELOAD_KEY]: '0.1.4' });

  reloadIfBundleIsStale('0.1.3', '0.1.4', storage, reload);

  expect(reload).not.toHaveBeenCalled();
});

it('should reload for a newer daemon version after an earlier reload', () => {
  const reload = vi.fn();
  const storage = makeStorage({ [STALE_BUNDLE_RELOAD_KEY]: '0.1.4' });

  reloadIfBundleIsStale('0.1.4', '0.1.5', storage, reload);

  expect(reload).toHaveBeenCalledTimes(1);
  expect(storage.getItem(STALE_BUNDLE_RELOAD_KEY)).toBe('0.1.5');
});

it('should not reload while the daemon version is unknown', () => {
  const reload = vi.fn();

  reloadIfBundleIsStale('0.1.3', undefined, makeStorage(), reload);

  expect(reload).not.toHaveBeenCalled();
});

it('should not reload a bundle built without a version', () => {
  const reload = vi.fn();

  reloadIfBundleIsStale(null, '0.1.4', makeStorage(), reload);

  expect(reload).not.toHaveBeenCalled();
});
