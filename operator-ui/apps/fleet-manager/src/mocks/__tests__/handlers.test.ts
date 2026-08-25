import { storeKey } from '@operator-ui/mock-devtools';
import { setupServer } from 'msw/node';
import { afterAll, afterEach, beforeAll, vi } from 'vitest';
import { handlers } from '@/mocks/handlers';
import { routeToKey } from '@/mocks/routes';
import { getState, setForcedError } from '@/mocks/state';
import { mockStore } from '@/mocks/store';
import { verbLog } from '@/mocks/verb-log';

const server = setupServer(...handlers);

// This Node runtime's built-in Web Storage API shadows jsdom's window.localStorage
// with a non-functional stub (its getItem/setItem are undefined), so the real
// persistence writes below would otherwise be silently swallowed by
// localStorageAdapter's try/catch. Stub a working in-memory Storage so the
// persistence tests can observe real writes; vi.unstubAllGlobals() restores it.
const createMemoryStorage = (): Storage => {
  const data = new Map<string, string>();
  return {
    getItem: (key: string) => data.get(key) ?? null,
    setItem: (key: string, value: string) => {
      data.set(key, value);
    },
    removeItem: (key: string) => {
      data.delete(key);
    },
    clear: () => data.clear(),
    key: (index: number) => Array.from(data.keys())[index] ?? null,
    get length() {
      return data.size;
    }
  } as Storage;
};

beforeAll(() => {
  server.listen({ onUnhandledRequest: 'error' });
  vi.stubGlobal('localStorage', createMemoryStorage());
});
afterEach(() => {
  server.resetHandlers();
  mockStore.reset();
});
afterAll(() => {
  server.close();
  vi.unstubAllGlobals();
});

const admin = async (body: unknown) => {
  const response = await fetch('http://localhost/api/admin', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body)
  });
  const text = await response.text();
  return { status: response.status, body: text ? JSON.parse(text) : null };
};

const login = (password = 'test-password') =>
  fetch('http://localhost/api/auth', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ password })
  });

const restoreBody = {
  OnboardFromBackup: {
    mnemonic:
      'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about',
    acknowledge_original_host_is_gone: true
  }
};

it('should reject an admin call before the operator has logged in', async () => {
  const { status } = await admin('ListSeats');

  expect(status).toBe(401);
});

it('should answer a unit-variant verb sent as a bare string', async () => {
  mockStore.setScenario('seats-mixed');
  await login();

  const { body } = await admin('ListSeats');

  expect(body.Ok.seats).toHaveLength(4);
});

it('should answer a struct-variant verb sent as a single-key object', async () => {
  mockStore.setScenario('seats-mixed');
  await login();

  const { body } = await admin({ SeatStatus: { seat_id: 'seat-running-01' } });

  expect(body.Ok.report.phase).toBe('running');
});

it('should return Err rather than a non-200 status for an unknown verb', async () => {
  await login();

  const { status, body } = await admin('NoSuchVerb');

  expect(status).toBe(200);
  expect(body.Err.kind).toBe('unparsable_request');
  expect(body.Err.message).toContain('unknown variant NoSuchVerb');
});

it('should refuse fleet verbs while the host is not onboarded', async () => {
  mockStore.setScenario('not-onboarded');
  await login();

  const { body } = await admin('ListSeats');

  expect(body.Err.kind).toBe('not_onboarded');
});

it('should reflect a mutation in a later read', async () => {
  mockStore.setScenario('seats-mixed');
  await login();

  await admin({ DecommissionSeat: { seat_id: 'seat-running-01' } });
  const { body } = await admin({ SeatStatus: { seat_id: 'seat-running-01' } });

  expect(body.Ok.decommissioned).toBe(true);
});

// Both admin() calls above hit the same in-process mockStore, so that test
// alone would pass even if persist() were deleted from the handler. This one
// reads the storage write directly — the appKey/key format is store.ts's
// (`operator-ui:dev:mocks:${appKey}`), so it fails if persist() stops running.
it('should persist a mutation to localStorage so a fresh read of it reflects the mutation', async () => {
  mockStore.setScenario('seats-mixed');
  await login();

  await admin({ DecommissionSeat: { seat_id: 'seat-running-01' } });

  const raw = window.localStorage.getItem(storeKey('fman'));
  const persisted = JSON.parse(raw as string) as {
    world: { seats: { seat_id: string; decommissioned: boolean }[] };
  };
  const seat = persisted.world.seats.find((s) => s.seat_id === 'seat-running-01');

  expect(seat?.decommissioned).toBe(true);
});

it('should not write to localStorage for a non-mutating verb', async () => {
  mockStore.setScenario('seats-mixed');
  await login();

  const before = window.localStorage.getItem(storeKey('fman'));
  await admin('ListSeats');
  const after = window.localStorage.getItem(storeKey('fman'));

  expect(after).toBe(before);
});

// A lost response is not a daemon refusal, so these four assert on the transport
// itself: `admin()` rejects only when fetch itself fails, which is what makes
// `adminCall` raise NetworkError rather than an AdminApiError.
it('should fail before dispatch without changing the world', async () => {
  mockStore.setScenario('not-onboarded');
  await login();
  getState().restoreTransport = 'fail-before-dispatch';

  await expect(admin(restoreBody)).rejects.toBeTruthy();
  expect(getState().onboarded).toBe(false);
});

it('should commit and then lose the answer', async () => {
  mockStore.setScenario('not-onboarded');
  await login();
  getState().restoreTransport = 'fail-after-commit';

  await expect(admin(restoreBody)).rejects.toBeTruthy();
  expect(getState().onboarded).toBe(true);
});

it('should fail the status check at the transport, not as a daemon error', async () => {
  mockStore.setScenario('fresh-fleet');
  await login();
  getState().onboardingTransport = 'network-failure';

  await expect(admin('Onboarding')).rejects.toBeTruthy();
});

it('should expire the session on the next recovery submit only', async () => {
  mockStore.setScenario('not-onboarded');
  await login();
  getState().restoreSession = 'expire-on-submit';

  const { status } = await admin(restoreBody);

  expect(status).toBe(401);
  expect(getState().onboarded).toBe(false);
  expect(getState().sessionActive).toBe(false);
  expect(getState().restoreSession).toBe('active');
});

// The read-error scenario carries its refusal as a seeded forced error, so this
// also proves a scenario can seed one through base().
it('should refuse the authorization read as a daemon error in the read-error scenario', async () => {
  mockStore.setScenario('authorization-read-error');
  await login();

  const { status, body } = await admin('Onboarding');

  expect(status).toBe(200);
  expect(body.Err.message).toContain('relay query failed');
});

it('should reject a login with the wrong password', async () => {
  const response = await login('wrong');

  expect(response.status).toBe(401);
});

// The panel's per-page tab is built entirely from what the handler records, so
// a handler that serves without recording would leave the tab permanently
// "listening" on a page that is in fact fetching.
it('should record a served verb against the route that was showing', async () => {
  await login();
  const routeKey = routeToKey(window.location.pathname) ?? 'unrouted';
  verbLog.clear(routeKey);

  await admin('ListSeats');

  expect(verbLog.list(routeKey)).toContain('ListSeats');
});

it('should record a verb the panel has forced into failing', async () => {
  await login();
  const routeKey = routeToKey(window.location.pathname) ?? 'unrouted';
  verbLog.clear(routeKey);
  setForcedError('ListSeats', 'unknown seat');

  const response = await admin('ListSeats');

  expect(response.body).toEqual({ Err: { kind: 'other', message: 'unknown seat' } });
  expect(verbLog.list(routeKey)).toContain('ListSeats');
  setForcedError('ListSeats', null);
});
