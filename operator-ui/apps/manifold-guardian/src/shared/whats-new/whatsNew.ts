import { useSyncExternalStore } from 'react';

/** One announcement in the Overview's "What's new" slot. */
export interface WhatsNewItem {
  /** Remembers "Not now"; never reuse an id for a different announcement. */
  id: string;
  title: string;
  body: string;
  action: { label: string; path: string };
}

// A preference of this browser, not of the MG: another browser shows the items again.
const SEEN_KEY = 'manifold-guardian.whats-new-seen';

const listeners = new Set<() => void>();

// Browsers that block storage throw here; items then show on every visit.
const readStored = () => {
  try {
    return window.localStorage.getItem(SEEN_KEY);
  } catch {
    return null;
  }
};

const parseSeen = (stored: string | null): string[] => {
  if (stored === null) return [];
  try {
    const parsed: unknown = JSON.parse(stored);
    return Array.isArray(parsed) ? parsed.filter((id) => typeof id === 'string') : [];
  } catch {
    return [];
  }
};

const subscribe = (listener: () => void) => {
  listeners.add(listener);
  window.addEventListener('storage', listener);
  return () => {
    listeners.delete(listener);
    window.removeEventListener('storage', listener);
  };
};

/** Hides one "What's new" item for good in this browser. */
export const markWhatsNewSeen = (id: string) => {
  const seen = parseSeen(readStored());
  if (seen.includes(id)) return;
  try {
    window.localStorage.setItem(SEEN_KEY, JSON.stringify([...seen, id]));
  } catch {
    return;
  }
  for (const listener of listeners) listener();
};

export const useWhatsNewSeen = (id: string): boolean =>
  parseSeen(useSyncExternalStore(subscribe, readStored)).includes(id);
