import type { NextFunction, Request, Response } from 'express';
import { getState } from '../../src/mocks/state';

// Mock stand-in for crates/fman/core/src/admin_http.rs, the HTTP adapter around the
// canonical crates/fman/core/src/admin.rs operator surface. The real adapter generates a
// fresh random HttpOnly cookie name+value per daemon process. A fixed name simplifies
// local debugging; it carries no real security meaning since this is a dev-only mock.
export const SESSION_COOKIE_NAME = 'fman_mock_session';
const SESSION_COOKIE_VALUE = 'mock-session-token';

export const startSession = (): void => {
  getState().sessionActive = true;
};

export const clearSession = (): void => {
  getState().sessionActive = false;
};

const readCookie = (req: Request, name: string): string | null => {
  const header = req.get('cookie') ?? '';
  const match = header
    .split(';')
    .map((part) => part.trim())
    .find((part) => part.startsWith(`${name}=`));
  return match ? match.slice(name.length + 1) : null;
};

// Real behavior (fedimint_ui_common::auth::require_auth): trusted-proxy mode performs no
// local auth at all; password mode requires the session cookie and responds with a bare
// 401 (no JSON body) when it is missing or wrong.
export const requireSession = (req: Request, res: Response, next: NextFunction): void => {
  const { authMode } = getState();
  if (authMode === 'trusted_proxy') {
    next();
    return;
  }
  if (getState().sessionActive && readCookie(req, SESSION_COOKIE_NAME) === SESSION_COOKIE_VALUE) {
    next();
    return;
  }
  res.status(401).end();
};

export const issueSessionCookie = (res: Response): void => {
  startSession();
  res.cookie(SESSION_COOKIE_NAME, SESSION_COOKIE_VALUE, {
    httpOnly: true,
    sameSite: 'lax'
  });
};

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

export const noStore = (_req: Request, res: Response, next: NextFunction): void => {
  res.set('Cache-Control', 'no-store');
  next();
};
