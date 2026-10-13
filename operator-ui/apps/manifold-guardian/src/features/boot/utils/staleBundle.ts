export const BUNDLE_VERSION: string | null = __MG_BUNDLE_VERSION__;

export const STALE_BUNDLE_RELOAD_KEY = 'mg:reloaded-for-daemon-version';

/**
 * Reload a tab whose dashboard code was built for a different daemon release.
 *
 * An open tab keeps running the bundle it loaded, so after an upgrade it can
 * misread the new daemon until someone reloads it (#154). One reload per daemon
 * version: if the reload still yields a mismatched bundle, a cache or proxy is
 * serving it, and reloading again would only loop.
 */
export const reloadIfBundleIsStale = (
  bundleVersion: string | null,
  daemonVersion: string | undefined,
  storage: Pick<Storage, 'getItem' | 'setItem'>,
  reload: () => void
): void => {
  if (bundleVersion === null || daemonVersion === undefined) return;
  if (daemonVersion === bundleVersion) return;
  if (storage.getItem(STALE_BUNDLE_RELOAD_KEY) === daemonVersion) return;

  storage.setItem(STALE_BUNDLE_RELOAD_KEY, daemonVersion);
  reload();
};
