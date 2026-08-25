import type { GetHealthResponse } from '@operator-ui/types';
import { Router, type Router as RouterType } from 'express';
import { getState } from '../../../src/mocks/state';
import { withRestoreMarker } from '../../../src/mocks/world/health';

// Unauthenticated liveness probe for the SPA boot sequence. Must serve the
// full GetHealthResponse (not a bare {status}) so isRestoreMode can read
// health.components before the operator has authenticated.
export const healthRouter: RouterType = Router();

healthRouter.get('/health', (_req, res) => {
  const { health, bootMode } = getState();
  const body: GetHealthResponse = withRestoreMarker(health, bootMode);
  res.json(body);
});
