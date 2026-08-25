import type { ServiceError } from '@operator-ui/types';
import {
  AccessDeniedError,
  AdminApiError,
  AuthError,
  DaemonUnavailableError,
  deferredRoutes,
  NetworkError,
  RouteDeferredError
} from './errors';
import { getToken } from './tokenStore';

const ADMIN_BASE = '/admin/v1';

const isServiceError = (value: unknown): value is ServiceError =>
  typeof value === 'object' &&
  value !== null &&
  typeof (value as ServiceError).code === 'string' &&
  typeof (value as ServiceError).message === 'string';

export const adminCall = async <Req, Res>(method: string, body: Req): Promise<Res> => {
  const headers: Record<string, string> = { 'content-type': 'application/json' };
  const authToken = getToken();
  if (authToken) headers.authorization = `Bearer ${authToken}`;

  let response: Response;
  try {
    response = await fetch(`${ADMIN_BASE}/${method}`, {
      method: 'POST',
      headers,
      body: JSON.stringify(body ?? null)
    });
  } catch {
    // fetch rejects only on network-level failure (daemon unreachable).
    throw new NetworkError();
  }

  if (response.ok) {
    return (await response.json()) as Res;
  }

  if (response.status === 401) {
    throw new AuthError();
  }

  let payload: unknown;
  try {
    payload = await response.json();
  } catch {
    payload = undefined;
  }

  if (isServiceError(payload)) {
    if (payload.code === 'unavailable') {
      if (deferredRoutes.has(method)) throw new RouteDeferredError(method, payload.message);
      // Still a NetworkError by inheritance, so every gate that reads that
      // keeps working — but a daemon that answered and refused is not a daemon
      // that could not be reached, and the operator is entitled to the
      // difference.
      throw new DaemonUnavailableError(payload.message);
    }
    // 401 (missing/invalid bearer) is handled above, so a permission_denied
    // reaching here is necessarily an authenticated request denied by policy
    // (403) — an access-denied state, never a re-auth trigger.
    if (payload.code === 'permission_denied') {
      throw new AccessDeniedError(payload.message);
    }
    throw new AdminApiError(payload.code, payload.message);
  }

  // 5xx / gateway errors without a ServiceError body → treat as daemon unreachable.
  throw new NetworkError(`HTTP ${response.status}`);
};
