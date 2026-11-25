import { expect, it } from 'vitest';
import { sweepRequestFor } from '../sweepRequest';

const pending = { method: 'lightning' as const, destination: 'old@example.com', id: 'request-1' };

it('should start a request when none is pending', () => {
  expect(sweepRequestFor(null, 'lightning', 'old@example.com')).toEqual({
    method: 'lightning',
    destination: 'old@example.com',
    id: expect.any(String)
  });
});

// A destination the dashboard has not read proves no change, and a new id after
// a lost response could start a second payout.
it.each([
  ['the destination is unchanged', pending, 'old@example.com'],
  ['the destination has not been read', pending, undefined],
  [
    'the first try went out before the destination was read',
    { method: 'lightning' as const, destination: undefined, id: 'request-1' },
    'new@example.com'
  ]
])('should keep the pending request when %s', (_case, current, destination) => {
  expect(sweepRequestFor(current, 'lightning', destination)).toBe(current);
});

it.each([
  ['another destination', 'new@example.com'],
  ['no destination', null]
])('should start a new request once the stored destination is %s', (_case, destination) => {
  const next = sweepRequestFor(pending, 'lightning', destination);

  expect(next).toEqual({ method: 'lightning', destination, id: expect.any(String) });
  expect(next.id).not.toBe('request-1');
});

// The daemon binds an id to its first method.
it('should start a new request when the method changes', () => {
  const next = sweepRequestFor(pending, 'ecash', 'old@example.com');

  expect(next).toMatchObject({ method: 'ecash' });
  expect(next.id).not.toBe('request-1');
});

// An ecash payout goes to no destination, so a saved address does not make a
// retry after a lost response a second payout.
it('should keep a pending ecash request when the destination changes', () => {
  const ecash = { method: 'ecash' as const, destination: null, id: 'request-2' };

  expect(sweepRequestFor(ecash, 'ecash', 'new@example.com')).toBe(ecash);
});
