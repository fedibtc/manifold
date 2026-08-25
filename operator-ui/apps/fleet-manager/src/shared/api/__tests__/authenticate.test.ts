import { afterEach, vi } from 'vitest';
import { authenticate, InvalidPasswordError } from '../authenticate';
import { HttpStatusError, NetworkError } from '../errors';

const originalFetch = global.fetch;

afterEach(() => {
  global.fetch = originalFetch;
  vi.restoreAllMocks();
});

it('should resolve on a 204 response', async () => {
  global.fetch = vi.fn().mockResolvedValue(new Response(null, { status: 204 }));

  await expect(authenticate('test-password')).resolves.toBeUndefined();
});

it('should POST the password as JSON with credentials included', async () => {
  const fetchMock = vi.fn().mockResolvedValue(new Response(null, { status: 204 }));
  global.fetch = fetchMock;

  await authenticate('test-password');

  expect(fetchMock).toHaveBeenCalledWith(
    '/api/auth',
    expect.objectContaining({
      method: 'POST',
      credentials: 'same-origin',
      body: JSON.stringify({ password: 'test-password' })
    })
  );
});

it('should throw InvalidPasswordError on a 401 response', async () => {
  global.fetch = vi.fn().mockResolvedValue(new Response(null, { status: 401 }));

  await expect(authenticate('wrong')).rejects.toBeInstanceOf(InvalidPasswordError);
});

it('should throw NetworkError when fetch rejects', async () => {
  global.fetch = vi.fn().mockRejectedValue(new TypeError('failed to fetch'));

  await expect(authenticate('test-password')).rejects.toBeInstanceOf(NetworkError);
});

it('should keep the original exception as the cause when fetch rejects', async () => {
  const cause = new TypeError('failed to fetch');
  global.fetch = vi.fn().mockRejectedValue(cause);

  await expect(authenticate('test-password')).rejects.toMatchObject({ cause });
});

it('should throw HttpStatusError, not InvalidPasswordError, on a non-401 failure', async () => {
  global.fetch = vi.fn().mockResolvedValue(new Response(null, { status: 500 }));

  const promise = authenticate('test-password');

  await expect(promise).rejects.toBeInstanceOf(HttpStatusError);
  await expect(promise).rejects.toMatchObject({ status: 500 });
});
