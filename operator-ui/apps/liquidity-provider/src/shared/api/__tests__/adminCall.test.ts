import { afterEach, beforeEach, vi } from 'vitest';
import { adminCall } from '../adminCall';
import {
  AccessDeniedError,
  AdminApiError,
  AuthError,
  deferredRoutes,
  NetworkError,
  RouteDeferredError
} from '../errors';
import { clearToken, setToken } from '../tokenStore';

const originalFetch = global.fetch;

const jsonResponse = (body: unknown, init?: ResponseInit): Response =>
  new Response(JSON.stringify(body), {
    status: 200,
    headers: { 'content-type': 'application/json' },
    ...init
  });

beforeEach(() => {
  clearToken();
});

afterEach(() => {
  global.fetch = originalFetch;
  vi.restoreAllMocks();
});

it('should POST to /admin/v1/<method> and return the parsed JSON body on 2xx', async () => {
  const fetchMock = vi.fn().mockResolvedValue(jsonResponse({ status: 'ready' }));
  global.fetch = fetchMock;

  const result = await adminCall('get_setup_state', null);

  expect(result).toEqual({ status: 'ready' });
  expect(fetchMock).toHaveBeenCalledWith(
    '/admin/v1/get_setup_state',
    expect.objectContaining({ method: 'POST', body: 'null' })
  );
});

it('should attach Authorization: Bearer <token> when a token is set', async () => {
  const fetchMock = vi.fn().mockResolvedValue(jsonResponse({}));
  global.fetch = fetchMock;
  setToken('secret-token');

  await adminCall('get_setup_state', null);

  const headers = fetchMock.mock.calls[0][1].headers as Record<string, string>;
  expect(headers.authorization).toBe('Bearer secret-token');
});

it('should omit the Authorization header when no token is set', async () => {
  const fetchMock = vi.fn().mockResolvedValue(jsonResponse({}));
  global.fetch = fetchMock;

  await adminCall('get_setup_state', null);

  const headers = fetchMock.mock.calls[0][1].headers as Record<string, string>;
  expect(headers.authorization).toBeUndefined();
});

it('should throw AuthError on HTTP 401', async () => {
  global.fetch = vi.fn().mockResolvedValue(jsonResponse({}, { status: 401 }));

  await expect(adminCall('get_setup_state', null)).rejects.toBeInstanceOf(AuthError);
});

it('should throw NetworkError when fetch rejects', async () => {
  global.fetch = vi.fn().mockRejectedValue(new TypeError('failed to fetch'));

  await expect(adminCall('get_setup_state', null)).rejects.toBeInstanceOf(NetworkError);
});

it('should throw RouteDeferredError when a deferred route returns ServiceError unavailable', async () => {
  const method = 'test_deferred_method';
  deferredRoutes.add(method);
  try {
    global.fetch = vi
      .fn()
      .mockResolvedValue(
        jsonResponse({ code: 'unavailable', message: 'not yet' }, { status: 503 })
      );

    const promise = adminCall(method, {});
    await expect(promise).rejects.toBeInstanceOf(RouteDeferredError);
    await expect(promise).rejects.toMatchObject({ method });
  } finally {
    deferredRoutes.delete(method);
  }
});

it('should throw NetworkError when a non-deferred method returns ServiceError unavailable', async () => {
  global.fetch = vi
    .fn()
    .mockResolvedValue(jsonResponse({ code: 'unavailable', message: 'down' }, { status: 503 }));

  await expect(adminCall('get_setup_state', null)).rejects.toBeInstanceOf(NetworkError);
});

it('should throw AdminApiError with the code on other ServiceError codes', async () => {
  global.fetch = vi
    .fn()
    .mockResolvedValue(
      jsonResponse({ code: 'invalid_argument', message: 'bad input' }, { status: 400 })
    );

  const promise = adminCall('apply_setup_config', {});
  await expect(promise).rejects.toBeInstanceOf(AdminApiError);
  await expect(promise).rejects.toMatchObject({ code: 'invalid_argument' });
});

it('should throw NetworkError on a 503 with no ServiceError body', async () => {
  global.fetch = vi.fn().mockResolvedValue(new Response('gateway down', { status: 503 }));

  await expect(adminCall('get_setup_state', null)).rejects.toBeInstanceOf(NetworkError);
});

it('should throw AccessDeniedError on a 403 with a permission_denied ServiceError body', async () => {
  global.fetch = vi
    .fn()
    .mockResolvedValue(
      jsonResponse({ code: 'permission_denied', message: 'not allowed' }, { status: 403 })
    );

  await expect(adminCall('get_setup_state', null)).rejects.toBeInstanceOf(AccessDeniedError);
});
