import { useEffect, useState } from 'react';

const DEFAULT_INTERVAL_MS = 60_000;

// A ticking "now" timestamp for relative-time display. Reading the clock in
// render is impure (react-hooks/purity) — instead seed from state and advance it
// on an interval from an effect, so render stays a pure function of state.
export const useNow = (intervalMs: number = DEFAULT_INTERVAL_MS): number => {
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(id);
  }, [intervalMs]);

  return now;
};
