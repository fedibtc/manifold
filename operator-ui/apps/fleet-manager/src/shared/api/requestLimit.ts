/**
 * A bound on how many calls of one kind may be in flight at once.
 *
 * Nothing is dropped: a call over the bound waits its turn and then runs, so a
 * large fleet spreads its fan-out over a few round trips instead of opening one
 * connection per seat at the same instant. Coverage is unchanged — only arrival
 * time is — so no screen has to declare a total incomplete because of this.
 */
export const createRequestLimit = (limit: number) => {
  let active = 0;
  const waiting: Array<() => void> = [];

  const acquire = (): Promise<void> => {
    if (active < limit) {
      active += 1;
      return Promise.resolve();
    }
    return new Promise((resolve) => {
      waiting.push(resolve);
    });
  };

  // The slot is handed straight to the next waiter rather than freed and
  // re-taken, so a caller arriving between the two cannot overtake the queue.
  const release = () => {
    const next = waiting.shift();
    if (next) {
      next();
      return;
    }
    active -= 1;
  };

  return async <T>(task: () => Promise<T>): Promise<T> => {
    await acquire();
    try {
      return await task();
    } finally {
      release();
    }
  };
};

// Browsers already cap connections per origin, but they cap them by queueing
// every request in the tab — including the one the operator just clicked. Four
// is a budget for background fan-out that leaves the rest of that allowance for
// foreground work.
export const SEAT_FAN_OUT_LIMIT = 4;

// One budget shared by every per-seat fan-out (seat reports and guardian fees):
// those run on different screens, but it is the same fleet and the same daemon.
export const seatFanOut = createRequestLimit(SEAT_FAN_OUT_LIMIT);
