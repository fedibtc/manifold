import { type Request, type Response, Router, type Router as RouterType } from 'express';
import { getState } from '../../../src/mocks/state';
import { issueSessionCookie } from '../middleware';

export const authRouter: RouterType = Router();

// Real behavior: trusted-proxy mode mounts no /api/auth route at all.
authRouter.post('/', (req: Request, res: Response) => {
  const { authMode, password } = getState();
  if (authMode === 'trusted_proxy') {
    res.status(404).end();
    return;
  }
  const { password: submitted } = (req.body ?? {}) as { password?: string };
  if (submitted !== password) {
    res.status(401).end();
    return;
  }
  issueSessionCookie(res);
  res.status(204).end();
});
