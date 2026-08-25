import { healthyComponents } from '@operator-ui/mock-fixtures';
import { Router, type Router as RouterType } from 'express';
import { getState } from '../../../src/mocks/state';
import { dispatch, isServiceErrorLike } from '../../../src/mocks/world/verbs';
import { DEFERRED_PHASE, resolveInjectedError, sendServiceError } from '../middleware';

export const adminRouter: RouterType = Router();

// Back-compat health (see routes/health.ts for the open SPA probe).
adminRouter.get('/health', (_req, res) => {
  res.json({ components: healthyComponents });
});

adminRouter.post('/:method', (req, res) => {
  const method = req.params.method;

  const injected = resolveInjectedError(method, req.query);
  if (injected) {
    sendServiceError(res, injected);
    return;
  }

  const requiredPhase = DEFERRED_PHASE[method];
  if (requiredPhase !== undefined && getState().phase < requiredPhase) {
    sendServiceError(res, 'unavailable', `method deferred to phase ${requiredPhase}`);
    return;
  }

  try {
    res.json(dispatch(method, req.body));
  } catch (error) {
    if (isServiceErrorLike(error)) {
      sendServiceError(res, error.code, error.message);
      return;
    }
    throw error;
  }
});
