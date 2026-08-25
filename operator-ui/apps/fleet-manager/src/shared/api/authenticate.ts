import { HttpStatusError, NetworkError } from './errors';

// POST /api/auth belongs to crates/fman/core/src/admin_http.rs, the HTTP adapter around
// the canonical crates/fman/core/src/admin.rs operator surface. It is not an
// AdminRequest verb, but a separate unauthenticated password-login route. Success is
// 204 No Content with a Set-Cookie the browser stores automatically; failure is a bare 401.
export class InvalidPasswordError extends Error {
  constructor(message = 'incorrect password') {
    super(message);
    this.name = 'InvalidPasswordError';
  }
}

export const authenticate = async (password: string): Promise<void> => {
  let response: Response;
  try {
    response = await fetch('/api/auth', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ password })
    });
  } catch (cause) {
    throw new NetworkError('network error', { cause });
  }

  // Only the 401 is the operator's password. Every other status is the fleet
  // manager or the path to it, and the sign-in form says so rather than
  // sending the operator to change a password that was never rejected.
  if (response.status === 401) {
    throw new InvalidPasswordError();
  }
  if (!response.ok) {
    throw new HttpStatusError(response.status);
  }
};
