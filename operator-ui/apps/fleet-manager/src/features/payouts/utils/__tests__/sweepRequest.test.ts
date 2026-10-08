import { expect, it } from 'vitest';
import { sweepRequestFor } from '../sweepRequest';

const pending = { destination: 'old@example.com', id: 'request-1' };

it('should start a request when none is pending', () => {
  expect(sweepRequestFor(null, 'old@example.com')).toEqual({
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
    { destination: undefined, id: 'request-1' },
    'new@example.com'
  ]
])('should keep the pending request when %s', (_case, current, destination) => {
  expect(sweepRequestFor(current, destination)).toBe(current);
});

it.each([
  ['another destination', 'new@example.com'],
  ['no destination', null]
])('should start a new request once the stored destination is %s', (_case, destination) => {
  const next = sweepRequestFor(pending, destination);

  expect(next).toEqual({ destination, id: expect.any(String) });
  expect(next.id).not.toBe('request-1');
});
