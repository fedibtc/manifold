import type { AdminErrorKind } from '@operator-ui/types';

// The transport never produced an answer: `fetch` itself rejected — connection
// refused, DNS, TLS, offline, aborted. There is no status to read because
// nothing was served. The original exception travels as `cause`: it is the only
// record left of which of those it was.
export class NetworkError extends Error {
  readonly kind = 'network';
  constructor(message = 'network error', options?: ErrorOptions) {
    super(message, options);
    this.name = 'NetworkError';
  }
}

export class AuthError extends Error {
  readonly kind = 'auth';
  constructor(message = 'unauthorized') {
    super(message);
    this.name = 'AuthError';
  }
}

// HTTP 403 on a request that carried a valid session. FM serves no 403 today;
// the class exists so that the day it does, the dashboard says "refused" rather
// than "unreachable", and never offers re-authentication — the credentials were
// accepted, the request was not.
export class AccessDeniedError extends Error {
  readonly kind = 'access_denied';
  constructor(message = 'permission denied') {
    super(message);
    this.name = 'AccessDeniedError';
  }
}

// Something answered with a status the admin API does not model as a Result: a
// 5xx from the daemon, or any status from a proxy in front of it. The status is
// the evidence, so it is carried rather than flattened into a message.
export class HttpStatusError extends Error {
  readonly kind = 'http';
  readonly status: number;
  constructor(status: number, options?: ErrorOptions) {
    super(`HTTP ${status}`, options);
    this.name = 'HttpStatusError';
    this.status = status;
  }
}

// A 2xx answer that is not the admin protocol: a body that will not decode, or
// one that decodes to neither `Ok` nor `Err` (a proxy serving index.html on
// /api/admin lands here). Neither a transport failure nor a daemon refusal — the
// decode exception is kept as `cause`.
export class ProtocolError extends Error {
  readonly kind = 'protocol';
  constructor(message = 'unreadable response', options?: ErrorOptions) {
    super(message, options);
    this.name = 'ProtocolError';
  }
}

// FM's admin API returns Result<Value, AdminError> (see
// crates/fman/core/src/admin.rs). `kind` on this class is this file's own
// classifier axis — which layer failed — so the daemon's own discriminant is
// carried beside it as `reason`. Branch on `reason`; the message is prose and
// may be reworded without notice.
//
// `reason` defaults to 'other' so a hand-built error (a test, a mock) need not
// invent one; anything that came off the wire carries what the daemon said.
export class AdminApiError extends Error {
  readonly kind = 'admin';
  readonly reason: AdminErrorKind;
  constructor(message: string, reason: AdminErrorKind = 'other') {
    super(message);
    this.name = 'AdminApiError';
    this.reason = reason;
  }
}

export type AdminCallError =
  | NetworkError
  | AuthError
  | AccessDeniedError
  | HttpStatusError
  | ProtocolError
  | AdminApiError;

// "The fleet manager is not serving this dashboard": the transport produced
// nothing, the answer was not the admin protocol at all, or the server side
// failed. A 401 or a 403 is deliberately excluded — the daemon answered, and it
// answered about the request, so blaming the connection would be a lie.
export const isDaemonUnreachable = (error: unknown): boolean =>
  error instanceof NetworkError ||
  error instanceof ProtocolError ||
  (error instanceof HttpStatusError && error.status >= 500);
