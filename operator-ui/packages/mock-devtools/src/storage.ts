import type { StorageAdapter } from './types';

// Persistence is deliberately behind three functions: the worlds measured at
// ~10 KiB make localStorage the right backend today, and if a fixture set ever
// grows past it, swapping the backend touches this file and nothing else.
export const localStorageAdapter: StorageAdapter = {
  load: (key) => {
    try {
      return window.localStorage.getItem(key);
    } catch {
      return null;
    }
  },
  save: (key, value) => {
    try {
      window.localStorage.setItem(key, value);
    } catch {
      // Quota or a privacy mode that blocks writes: the in-memory world still
      // works for this session, so a failed write must not break the app.
    }
  }
};
