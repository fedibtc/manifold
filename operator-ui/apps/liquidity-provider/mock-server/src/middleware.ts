import type { ServiceError, ServiceErrorCode } from '@operator-ui/types';
import type { NextFunction, Request, Response } from 'express';
import { getState } from '../../src/mocks/state';

export type InjectableError = ServiceErrorCode | '503';

// permission_denied → 403 (an authenticated request denied by policy), per
// SPEC-flip-admin-api.md:31-33 and admin_http.rs's status_for_error. Missing
// or invalid bearer is a separate, always-401 case handled in bearerAuth below
// — it never goes through this table.
const HTTP_STATUS: Record<ServiceErrorCode, number> = {
  invalid_argument: 400,
  failed_precondition: 400,
  permission_denied: 403,
  not_found: 404,
  unavailable: 503,
  internal: 500,
  unknown: 500
};

const DEFAULT_MESSAGE: Record<ServiceErrorCode, string> = {
  invalid_argument: 'invalid argument',
  failed_precondition: 'failed precondition',
  permission_denied: 'permission denied',
  not_found: 'not found',
  unavailable: 'route not available in mock',
  internal: 'internal error',
  unknown: 'unknown error'
};

// Methods deferred to a later daemon phase; below that phase they are
// unavailable. Empty until the next method is gated.
export const DEFERRED_PHASE: Record<string, number> = {};

export const errorHttpStatus = (code: InjectableError): number =>
  code === '503' ? 503 : HTTP_STATUS[code];

export const sendServiceError = (res: Response, code: InjectableError, message?: string): void => {
  const resolvedCode: ServiceErrorCode = code === '503' ? 'unavailable' : code;
  const body: ServiceError = {
    code: resolvedCode,
    message: message ?? DEFAULT_MESSAGE[resolvedCode]
  };
  res.status(errorHttpStatus(code)).json(body);
};

const isInjectable = (value: unknown): value is InjectableError =>
  value === '503' || (typeof value === 'string' && value in HTTP_STATUS);

// Query `?error=` wins; otherwise the forcedErrors[method] entry.
export const resolveInjectedError = (
  method: string,
  query: Request['query']
): InjectableError | null => {
  const raw = query.error;
  if (typeof raw === 'string' && isInjectable(raw)) return raw;
  const forced = getState().forcedErrors[method];
  return forced ?? null;
};

// Bearer auth for /admin/v1/* except GET /admin/v1/health.
export const bearerAuth = (req: Request, res: Response, next: NextFunction): void => {
  if (req.method === 'GET' && req.path === '/health') {
    next();
    return;
  }
  const header = req.get('authorization') ?? '';
  const token = header.startsWith('Bearer ') ? header.slice(7).trim() : '';
  if (!token) {
    // Missing/invalid bearer is always 401, regardless of the permission_denied
    // code it carries — matches admin_http.rs's require_auth_token, which fixes
    // this case to StatusCode::UNAUTHORIZED ahead of status_for_error.
    res
      .status(401)
      .json({ code: 'permission_denied', message: 'missing bearer token' } satisfies ServiceError);
    return;
  }
  next();
};

// Simulated latency before responding, driven by state.latencyMs.
export const applyLatency = async (
  _req: Request,
  _res: Response,
  next: NextFunction
): Promise<void> => {
  const { latencyMs } = getState();
  if (latencyMs > 0) {
    await new Promise((resolve) => setTimeout(resolve, latencyMs));
  }
  next();
};
