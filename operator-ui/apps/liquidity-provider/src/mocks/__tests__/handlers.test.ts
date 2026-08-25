import { storeKey } from '@operator-ui/mock-devtools';
import { setupServer } from 'msw/node';
import { afterAll, afterEach, beforeAll, vi } from 'vitest';
import { handlers } from '@/mocks/handlers';
import { routeToKey } from '@/mocks/routes';
import { setForcedError } from '@/mocks/state';
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

const admin = async (method: string, body: unknown = null, token = 'e2e-token') => {
  const response = await fetch(`http://localhost/admin/v1/${method}`, {
    method: 'POST',
    headers: { 'content-type': 'application/json', authorization: `Bearer ${token}` },
    body: JSON.stringify(body)
  });
  const text = await response.text();
  return { status: response.status, body: text ? JSON.parse(text) : null };
};

it('should serve the unauthenticated health probe', async () => {
  const response = await fetch('http://localhost/health');

  expect(response.status).toBe(200);
  expect((await response.json()).components).toBeDefined();
});

it('should reject an admin call with no bearer token', async () => {
  const response = await fetch('http://localhost/admin/v1/get_funds', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: 'null'
  });

  expect(response.status).toBe(401);
});

it('should answer get_setup_state for the default scenario', async () => {
  const { status, body } = await admin('get_setup_state');

  expect(status).toBe(200);
  expect(body.status).toBe('not_configured');
});

it('should report a configured setup once the ready scenario is active', async () => {
  mockStore.setScenario('all-clear');

  const { body } = await admin('get_setup_state');

  expect(body.status).toBe('ready');
});

// dispatch() pins unknown methods to unavailable/503 (see world/verbs.ts and
// its regression test) so adminCall.ts can tell the G1 boot gate apart from an
// ordinary in-app error; matches the pre-existing Express behaviour too.
it('should answer with a service error for an unknown method', async () => {
  const { status, body } = await admin('no_such_method');

  expect(status).toBe(503);
  expect(body.code).toBe('unavailable');
});

it('should honour the error query override', async () => {
  const response = await fetch('http://localhost/admin/v1/get_funds?error=unavailable', {
    method: 'POST',
    headers: { 'content-type': 'application/json', authorization: 'Bearer e2e-token' },
    body: 'null'
  });

  expect(response.status).toBe(503);
});

it('should answer permission_denied with 403 for an authenticated request', async () => {
  // Distinct from the no-bearer-token case above (always 401): a permission_denied
  // ServiceError on a request that *did* carry a bearer token is an authenticated
  // request denied by policy, per SPEC-flip-admin-api.md:31-33 / admin_http.rs.
  const response = await fetch('http://localhost/admin/v1/get_funds?error=permission_denied', {
    method: 'POST',
    headers: { 'content-type': 'application/json', authorization: 'Bearer e2e-token' },
    body: 'null'
  });

  expect(response.status).toBe(403);
  expect((await response.json()).code).toBe('permission_denied');
});

it('should persist a mutation to localStorage', async () => {
  mockStore.setScenario('all-clear');

  await admin('withdraw_advertisement');
  const raw = window.localStorage.getItem(storeKey('flip'));

  expect(JSON.parse(raw ?? '{}').world.advertisement.publicationStatus).toBe('withdrawn');
});

// The panel's per-page tab is built entirely from what the handler records, so
// a handler that serves without recording would leave the tab permanently
// "listening" on a page that is in fact fetching.
it('should record a served verb against the route that was showing', async () => {
  const routeKey = routeToKey(window.location.pathname) ?? 'unrouted';
  verbLog.clear(routeKey);

  await admin('get_funds');

  expect(verbLog.list(routeKey)).toContain('get_funds');
});

it('should record a verb the panel has forced into failing', async () => {
  const routeKey = routeToKey(window.location.pathname) ?? 'unrouted';
  verbLog.clear(routeKey);
  setForcedError('get_funds', 'unavailable');

  const response = await admin('get_funds');

  expect(response.status).toBe(503);
  expect(verbLog.list(routeKey)).toContain('get_funds');
  setForcedError('get_funds', null);
});

// The mock has to refuse what the daemon refuses, or a dashboard test passes
// against a world the real thing rejects. That is what let the empty-secret
// misreading survive: the mock never modelled the constraint.
it('should refuse an empty secret rather than treating it as a removal', async () => {
  const refused = await admin('set_config_secret', {
    secret: 'chain_observer_password',
    update: { action: 'set', value: '' }
  });

  expect(refused.status).toBe(400);
  expect(refused.body.code).toBe('invalid_argument');
});

it('should refuse to clear the gateway credential and allow clearing the password', async () => {
  const gateway = await admin('set_config_secret', {
    secret: 'gateway_admin_credential',
    update: { action: 'clear' }
  });
  expect(gateway.status).toBe(400);

  const password = await admin('set_config_secret', {
    secret: 'chain_observer_password',
    update: { action: 'clear' }
  });
  expect(password.status).toBe(200);
  expect(password.body).toEqual({ secret: 'chain_observer_password', present: false });
});
