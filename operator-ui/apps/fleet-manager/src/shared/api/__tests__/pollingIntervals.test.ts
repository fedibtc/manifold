import { afterEach, beforeEach, vi } from 'vitest';
import { type BackoffPolicy, pollIntervalMs, withJitter } from '../pollingIntervals';

const POLICY: BackoffPolicy = { baseMs: 5_000, healthyMs: 60_000, ceilingMs: 60_000 };

// `errorUpdateCount` is react-query's lifetime error count: it grows by one per
// failure and never falls back on a recovery. The streak is what these cases are
// about, so each one drives that count the way a real query would.
const failed = (errorUpdateCount: number) => ({ status: 'error' as const, errorUpdateCount });
const succeeded = (errorUpdateCount: number) => ({ status: 'success' as const, errorUpdateCount });
// A retry in flight. react-query clears the error of a query that holds no data
// when the next fetch starts, so this state is indistinguishable from a healthy
// one by `error` alone — which is why the streak reads `status`.
const refetching = (errorUpdateCount: number) => ({
  status: 'pending' as const,
  errorUpdateCount
});

// Math.random is stubbed to the midpoint everywhere below, which makes the
// jitter term exactly zero and the intervals assertable as literals.
beforeEach(() => {
  vi.spyOn(Math, 'random').mockReturnValue(0.5);
});

afterEach(() => {
  vi.restoreAllMocks();
});

it('should poll at the healthy cadence while the last poll succeeded', () => {
  expect(pollIntervalMs(succeeded(0), POLICY, 'healthy')).toBe(60_000);
});

it('should retry promptly at the base cadence on the first failure', () => {
  expect(pollIntervalMs(failed(1), POLICY, 'first')).toBe(5_000);
});

it('should grow the interval with each consecutive failure', () => {
  expect(pollIntervalMs(failed(1), POLICY, 'grow')).toBe(5_000);
  expect(pollIntervalMs(failed(2), POLICY, 'grow')).toBe(10_000);
  expect(pollIntervalMs(failed(3), POLICY, 'grow')).toBe(20_000);
  expect(pollIntervalMs(failed(4), POLICY, 'grow')).toBe(40_000);
});

it('should stop growing at the ceiling', () => {
  pollIntervalMs(failed(1), POLICY, 'ceiling');
  pollIntervalMs(failed(2), POLICY, 'ceiling');
  pollIntervalMs(failed(3), POLICY, 'ceiling');

  expect(pollIntervalMs(failed(4), POLICY, 'ceiling')).toBe(40_000);
  expect(pollIntervalMs(failed(5), POLICY, 'ceiling')).toBe(60_000);
  expect(pollIntervalMs(failed(6), POLICY, 'ceiling')).toBe(60_000);
});

// The whole point of a streak rather than a lifetime error count: a query that
// recovers is owed the same prompt retry on its next failure as one that has
// never failed, however long the run of failures before the recovery was.
it('should reset the streak on a success and retry promptly on the next failure', () => {
  expect(pollIntervalMs(succeeded(0), POLICY, 'sequence')).toBe(60_000);
  expect(pollIntervalMs(failed(1), POLICY, 'sequence')).toBe(5_000);
  expect(pollIntervalMs(failed(2), POLICY, 'sequence')).toBe(10_000);
  expect(pollIntervalMs(failed(3), POLICY, 'sequence')).toBe(20_000);

  expect(pollIntervalMs(succeeded(3), POLICY, 'sequence')).toBe(60_000);

  expect(pollIntervalMs(failed(4), POLICY, 'sequence')).toBe(5_000);
  expect(pollIntervalMs(failed(5), POLICY, 'sequence')).toBe(10_000);
});

// react-query recomputes the interval on every render, not once per fetch. A
// streak that counted calls would grow with rendering — a screen that renders
// often would back its poll off to the ceiling without a single extra failure.
it('should not advance the streak when the same failure is evaluated again', () => {
  const state = failed(1);

  expect(pollIntervalMs(state, POLICY, 'idempotent')).toBe(5_000);
  expect(pollIntervalMs(state, POLICY, 'idempotent')).toBe(5_000);
  expect(pollIntervalMs(state, POLICY, 'idempotent')).toBe(5_000);
});

// Regression: read through `error` instead of `status`, a retry in flight looked
// like a recovery — the streak reset on every attempt, and a poll that never
// succeeded kept the prompt first-failure cadence for the life of the tab.
it('should hold the streak through a retry that has not answered yet', () => {
  expect(pollIntervalMs(failed(1), POLICY, 'in-flight')).toBe(5_000);
  expect(pollIntervalMs(refetching(1), POLICY, 'in-flight')).toBe(5_000);

  expect(pollIntervalMs(failed(2), POLICY, 'in-flight')).toBe(10_000);
  expect(pollIntervalMs(refetching(2), POLICY, 'in-flight')).toBe(10_000);

  expect(pollIntervalMs(failed(3), POLICY, 'in-flight')).toBe(20_000);
});

// The streak outlives any one query — it is keyed by seed, and a remount or a
// fresh cache reuses that seed with an error count that starts over.
it('should start the streak over for a different query under the same seed', () => {
  expect(pollIntervalMs(failed(1), POLICY, 'remount')).toBe(5_000);
  expect(pollIntervalMs(failed(2), POLICY, 'remount')).toBe(10_000);

  expect(pollIntervalMs(failed(1), POLICY, 'remount')).toBe(5_000);
});

it('should spread the interval either side of the requested cadence', () => {
  vi.spyOn(Math, 'random').mockReturnValue(0);
  expect(withJitter(10_000, 'low')).toBe(8_000);

  vi.spyOn(Math, 'random').mockReturnValue(1);
  expect(withJitter(10_000, 'high')).toBe(12_000);
});

it('should spread two pollers apart from each other', () => {
  vi.spyOn(Math, 'random').mockReturnValueOnce(0).mockReturnValueOnce(1);

  expect(withJitter(10_000, 'seat-a')).not.toBe(withJitter(10_000, 'seat-b'));
});

// react-query clears and restarts a poll timer whenever the interval it computes
// differs from the running one, and it recomputes on every render. An interval
// redrawn per call would reset the timer on every render, so a screen rendering
// faster than it polls would never poll at all.
it('should return the same interval for the same poller so the timer is not reset', () => {
  vi.spyOn(Math, 'random').mockReturnValue(0.31);

  const first = withJitter(10_000, 'stable');
  const second = withJitter(10_000, 'stable');

  expect(second).toBe(first);
});
