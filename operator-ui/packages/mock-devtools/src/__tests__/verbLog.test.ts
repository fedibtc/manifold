import { createVerbLog } from '../verb-log';

const atRoute = (routeKey: { current: string }) => createVerbLog(() => routeKey.current);

it('should list a verb under the route that was showing when it was served', () => {
  const route = { current: 'seats' };
  const log = atRoute(route);

  log.record('ListSeats');

  expect(log.list('seats')).toEqual(['ListSeats']);
});

it('should keep each route key to its own verbs', () => {
  const route = { current: 'seats' };
  const log = atRoute(route);
  log.record('ListSeats');

  route.current = 'wallet';
  log.record('ListPaymentFederations');

  expect(log.list('seats')).toEqual(['ListSeats']);
  expect(log.list('wallet')).toEqual(['ListPaymentFederations']);
});

it('should record a verb once however often it is served', () => {
  const log = atRoute({ current: 'seats' });

  log.record('ListSeats');
  log.record('ListSeats');

  expect(log.list('seats')).toEqual(['ListSeats']);
});

it('should keep verbs in the order they were first served', () => {
  const log = atRoute({ current: 'seats' });

  log.record('ListSeats');
  log.record('GuardianFees');
  log.record('ListSeats');

  expect(log.list('seats')).toEqual(['ListSeats', 'GuardianFees']);
});

it('should return an empty list for a route nothing has been served on', () => {
  const log = atRoute({ current: 'seats' });

  expect(log.list('offer')).toEqual([]);
});

it('should return the same reference for an unseen route across calls', () => {
  const log = atRoute({ current: 'seats' });

  expect(log.list('offer')).toBe(log.list('backup'));
});

it('should return the same reference while a route gains no new verb', () => {
  const log = atRoute({ current: 'seats' });
  log.record('ListSeats');
  const first = log.list('seats');

  log.record('ListSeats');

  expect(log.list('seats')).toBe(first);
});

it('should notify subscribers when a new verb is served', () => {
  const log = atRoute({ current: 'seats' });
  let calls = 0;
  log.subscribe(() => {
    calls += 1;
  });

  log.record('ListSeats');

  expect(calls).toBe(1);
});

it('should not notify subscribers when a verb repeats', () => {
  const log = atRoute({ current: 'seats' });
  log.record('ListSeats');
  let calls = 0;
  log.subscribe(() => {
    calls += 1;
  });

  log.record('ListSeats');

  expect(calls).toBe(0);
});

it('should stop notifying once a subscriber unsubscribes', () => {
  const log = atRoute({ current: 'seats' });
  let calls = 0;
  const unsubscribe = log.subscribe(() => {
    calls += 1;
  });

  unsubscribe();
  log.record('ListSeats');

  expect(calls).toBe(0);
});

it('should drop the verbs recorded for a cleared route', () => {
  const log = atRoute({ current: 'seats' });
  log.record('ListSeats');

  log.clear('seats');

  expect(log.list('seats')).toEqual([]);
});

it('should notify subscribers when a route is cleared', () => {
  const log = atRoute({ current: 'seats' });
  log.record('ListSeats');
  let calls = 0;
  log.subscribe(() => {
    calls += 1;
  });

  log.clear('seats');

  expect(calls).toBe(1);
});

it('should not notify when clearing a route that recorded nothing', () => {
  const log = atRoute({ current: 'seats' });
  let calls = 0;
  log.subscribe(() => {
    calls += 1;
  });

  log.clear('offer');

  expect(calls).toBe(0);
});
