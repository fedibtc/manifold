import type { AdminRequest, AdminResult } from '@operator-ui/types';
import {
  AccessDeniedError,
  AdminApiError,
  AuthError,
  HttpStatusError,
  NetworkError,
  ProtocolError
} from './errors';

export const ADMIN_ROUTE = '/api/admin';

// A call that never settles is not a slow call, it is a stuck one, and it costs
// more than its own screen: the per-seat fan-outs share a bounded set of slots
// (see requestLimit.ts), and a slot is only released when its call finishes. A
// handful of sockets that neither answer nor fail — a black-holed connection,
// a proxy that accepted and went quiet — would otherwise hold every slot and
// stop seat and fee polling everywhere, silently, until the browser gave up on
// its own schedule.
//
// Longer than any healthy admin response and shorter than the browser's own
// patience, so the failure is ours to classify rather than something the
// operator waits out. React Query does not start a second fetch for a key while
// one is in flight, so this cannot stack up behind the 5-second seat cadence.
export const ADMIN_REQUEST_TIMEOUT_MS = 15_000;

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === 'object' && value !== null;

// crates/fman/core/src/admin_http.rs, the HTTP adapter around
// crates/fman/core/src/admin.rs, sets FM's session as an HttpOnly cookie via
// POST /api/auth. There is nothing for the client to read or store; the browser
// attaches it automatically via credentials: 'same-origin'.
//
// Every failure keeps the evidence it arrived with — the original exception as
// `cause`, the real status on HttpStatusError. A dead daemon, a 500 and a 403
// are different facts and the screens above this call read them apart.
export const adminCall = async <Res>(request: AdminRequest): Promise<Res> => {
  let response: Response;
  try {
    response = await fetch(ADMIN_ROUTE, {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(request),
      signal: AbortSignal.timeout(ADMIN_REQUEST_TIMEOUT_MS)
    });
  } catch (cause) {
    // A timeout lands here too, and belongs here: nothing was served, so the
    // fact is the same one a refused connection reports. The cause carries
    // which it was.
    throw new NetworkError('network error', { cause });
  }

  if (response.status === 401) {
    throw new AuthError();
  }

  if (response.status === 403) {
    throw new AccessDeniedError();
  }

  if (!response.ok) {
    throw new HttpStatusError(response.status);
  }

  let payload: unknown;
  try {
    payload = await response.json();
  } catch (cause) {
    // A decode failure used to escape this function raw, as a SyntaxError no
    // classifier downstream recognised.
    throw new ProtocolError('response body did not decode as JSON', { cause });
  }

  if (!isRecord(payload)) {
    throw new ProtocolError('response body was not an admin result');
  }

  const result = payload as AdminResult<Res>;
  if ('Err' in result) {
    // The daemon serves `{ kind, message }`. Anything else on the Err side is
    // not this protocol, and saying so beats inventing a message from it.
    if (!isRecord(result.Err) || typeof result.Err.message !== 'string') {
      throw new ProtocolError('admin error was not { kind, message }');
    }
    throw new AdminApiError(result.Err.message, result.Err.kind);
  }
  if (!('Ok' in result)) {
    throw new ProtocolError('response carried neither Ok nor Err');
  }
  return result.Ok;
};
