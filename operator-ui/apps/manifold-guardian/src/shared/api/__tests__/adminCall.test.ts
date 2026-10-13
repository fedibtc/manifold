import { afterEach, vi } from 'vitest';
import { ADMIN_REQUEST_TIMEOUT_MS, adminCall } from '../adminCall';
import {
  AccessDeniedError,
  AdminApiError,
  AuthError,
  HttpStatusError,
  isDaemonUnreachable,
  NetworkError,
  ProtocolError
} from '../errors';

const originalFetch = global.fetch;

const jsonResponse = (body: unknown, init?: ResponseInit): Response =>
  new Response(JSON.stringify(body), {
    status: 200,
    headers: { 'content-type': 'application/json' },
    ...init
  });

afterEach(() => {
  global.fetch = originalFetch;
  vi.restoreAllMocks();
});

it('should POST a unit-variant request as a bare JSON string', async () => {
  const fetchMock = vi.fn().mockResolvedValue(jsonResponse({ Ok: { plans: [] } }));
  global.fetch = fetchMock;

  const result = await adminCall('ShowPlans');

  expect(result).toEqual({ plans: [] });
  expect(fetchMock).toHaveBeenCalledWith(
    '/api/admin',
    expect.objectContaining({ method: 'POST', credentials: 'same-origin', body: '"ShowPlans"' })
  );
});

it('should POST a struct-variant request as a single-key object', async () => {
  const fetchMock = vi
    .fn()
    .mockImplementation(() => Promise.resolve(jsonResponse({ Ok: { seats: [] } })));
  global.fetch = fetchMock;

  await adminCall('ListSeats');
  await adminCall({ SeatStatus: { seat_id: 'abc' } });

  expect(fetchMock.mock.calls[1][1]).toEqual(
    expect.objectContaining({ body: JSON.stringify({ SeatStatus: { seat_id: 'abc' } }) })
  );
});

it('should return the Ok payload on success', async () => {
  global.fetch = vi.fn().mockResolvedValue(jsonResponse({ Ok: { mnemonic: 'a b c' } }));

  const result = await adminCall('ShowMnemonic');

  expect(result).toEqual({ mnemonic: 'a b c' });
});

it('should throw AdminApiError with the Err message and reason on a 200 Err envelope', async () => {
  global.fetch = vi
    .fn()
    .mockResolvedValue(jsonResponse({ Err: { kind: 'other', message: 'unknown seat' } }));

  const promise = adminCall({ SeatStatus: { seat_id: 'missing' } });

  await expect(promise).rejects.toBeInstanceOf(AdminApiError);
  await expect(promise).rejects.toMatchObject({ message: 'unknown seat', reason: 'other' });
});

// The daemon's own discriminant is what a screen selects an action from, so it
// has to survive the transport rather than be re-derived from the sentence.
it('should carry the daemon reason through to the thrown error', async () => {
  global.fetch = vi.fn().mockResolvedValue(
    jsonResponse({
      Err: { kind: 'seat_directory_exists', message: 'seat 07 would be restored over a directory' }
    })
  );

  const promise = adminCall({
    OnboardFromBackup: { mnemonic: 'x', acknowledge_original_host_is_gone: true }
  });

  await expect(promise).rejects.toMatchObject({ reason: 'seat_directory_exists' });
});

// A body that is not this protocol is not a daemon refusal, and reading a
// message out of it would invent one.
it('should raise ProtocolError when the Err side is not { kind, message }', async () => {
  global.fetch = vi.fn().mockResolvedValue(jsonResponse({ Err: 'unknown seat' }));

  await expect(adminCall('ShowPlans')).rejects.toBeInstanceOf(ProtocolError);
});

it('should throw AuthError on HTTP 401', async () => {
  global.fetch = vi.fn().mockResolvedValue(jsonResponse({}, { status: 401 }));

  await expect(adminCall('ListSeats')).rejects.toBeInstanceOf(AuthError);
});

it('should throw NetworkError when fetch rejects', async () => {
  global.fetch = vi.fn().mockRejectedValue(new TypeError('failed to fetch'));

  await expect(adminCall('ListSeats')).rejects.toBeInstanceOf(NetworkError);
});

it('should keep the original exception as the cause when fetch rejects', async () => {
  const cause = new TypeError('failed to fetch');
  global.fetch = vi.fn().mockRejectedValue(cause);

  await expect(adminCall('ListSeats')).rejects.toMatchObject({ cause });
});

it('should throw HttpStatusError carrying the real status on a non-2xx response', async () => {
  global.fetch = vi.fn().mockResolvedValue(new Response('gateway down', { status: 502 }));

  const promise = adminCall('ListSeats');

  await expect(promise).rejects.toBeInstanceOf(HttpStatusError);
  await expect(promise).rejects.toMatchObject({ status: 502 });
});

it('should throw AccessDeniedError on HTTP 403, which must not read as unreachable', async () => {
  global.fetch = vi.fn().mockResolvedValue(new Response(null, { status: 403 }));

  const error = await adminCall('ListSeats').catch((thrown: unknown) => thrown);

  expect(error).toBeInstanceOf(AccessDeniedError);
  expect(isDaemonUnreachable(error)).toBe(false);
});

it('should throw ProtocolError when a 2xx body does not decode', async () => {
  global.fetch = vi.fn().mockResolvedValue(new Response('<!doctype html>', { status: 200 }));

  const error = await adminCall('ListSeats').catch((thrown: unknown) => thrown);

  expect(error).toBeInstanceOf(ProtocolError);
  expect((error as ProtocolError).cause).toBeInstanceOf(Error);
});

it('should throw ProtocolError when a decoded body carries neither Ok nor Err', async () => {
  global.fetch = vi.fn().mockResolvedValue(jsonResponse({ seats: [] }));

  await expect(adminCall('ListSeats')).rejects.toBeInstanceOf(ProtocolError);
});

it('should abort a request that never settles, so it cannot hold a fan-out slot', async () => {
  // The per-seat fan-outs share a bounded set of slots and only free one when
  // the call finishes. Without this bound, a socket that neither answers nor
  // fails would hold its slot and stop seat and fee polling everywhere.
  const fetchMock = vi.fn(
    (_input: unknown, init?: RequestInit) =>
      new Promise<Response>((_resolve, reject) => {
        init?.signal?.addEventListener('abort', () => reject(init.signal?.reason));
      })
  );
  global.fetch = fetchMock as unknown as typeof fetch;

  vi.useFakeTimers();
  try {
    const pending = adminCall('ShowPlans');
    const asserted = expect(pending).rejects.toBeInstanceOf(NetworkError);
    await vi.advanceTimersByTimeAsync(ADMIN_REQUEST_TIMEOUT_MS);
    await asserted;
  } finally {
    vi.useRealTimers();
  }

  expect(fetchMock.mock.calls[0]?.[1]?.signal).toBeInstanceOf(AbortSignal);
});
