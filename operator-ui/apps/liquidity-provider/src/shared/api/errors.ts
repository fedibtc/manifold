import type { ServiceErrorCode } from '@operator-ui/types';

// Admin methods that are phase-gated in the daemon. When one of these returns
// ServiceError { code: 'unavailable' }, it is a deferred route, not a dead
// daemon — surfaced distinctly so the UI can show "not available yet" instead
// of the G1 daemon-unreachable screen. Empty until the next method is gated.
export const deferredRoutes = new Set<string>();

export class NetworkError extends Error {
  readonly kind = 'network';
  constructor(message = 'network error') {
    super(message);
    this.name = 'NetworkError';
  }
}

// A daemon that answered and refused, rather than one that did not answer.
//
// The daemon returns ServiceError { code: 'unavailable' } while it has no
// runtime to serve from — a live restore swapping the data dir, or a start that
// has not built its first generation. It is a deliberate "not right now" from a
// process that is up, and it ends on its own.
//
// It extends NetworkError so every existing `instanceof NetworkError` check
// keeps its behaviour: this is still the class of failure that gates boot and
// keeps a screen from claiming fresh data. Only the sentence the operator reads
// changes, and only where the distinction is worth drawing.
export class DaemonUnavailableError extends NetworkError {
  constructor(message = 'the daemon is not serving requests right now') {
    super(message);
    this.name = 'DaemonUnavailableError';
  }
}

export class AuthError extends Error {
  readonly kind = 'auth';
  constructor(message = 'unauthorized') {
    super(message);
    this.name = 'AuthError';
  }
}

export class RouteDeferredError extends Error {
  readonly kind = 'route_deferred';
  readonly method: string;
  constructor(method: string, message = 'route not available yet') {
    super(message);
    this.name = 'RouteDeferredError';
    this.method = method;
  }
}

export class AdminApiError extends Error {
  readonly kind = 'admin';
  readonly code: ServiceErrorCode;
  constructor(code: ServiceErrorCode, message: string) {
    super(message);
    this.name = 'AdminApiError';
    this.code = code;
  }
}

// A permission_denied ServiceError on an AUTHENTICATED request (HTTP 403), per
// SPEC-flip-admin-api.md:31-33. Distinct from AuthError (401, missing/invalid
// bearer): this is not a credentials problem, so it must never trigger re-auth.
export class AccessDeniedError extends Error {
  readonly kind = 'access_denied';
  constructor(message = 'permission denied') {
    super(message);
    this.name = 'AccessDeniedError';
  }
}

export type AdminCallError =
  | NetworkError
  | AuthError
  | RouteDeferredError
  | AdminApiError
  | AccessDeniedError;
