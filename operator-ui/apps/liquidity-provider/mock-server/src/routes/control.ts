import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { type Request, type Response, Router, type Router as RouterType } from 'express';
import { hasScenario } from '../../../src/mocks/scenarios';
import {
  getState,
  type MockState,
  type PatchInput,
  patchState,
  resetState,
  setState,
  tick
} from '../../../src/mocks/state';
import type { InjectableError } from '../middleware';
import { sendServiceError } from '../middleware';

// Dev-only control surface. Open (no auth). Never shipped to production.
export const controlRouter: RouterType = Router();

// Static control panel, read once at startup (dev-only tool, no hot reload).
const controlUiPath = join(dirname(fileURLToPath(import.meta.url)), '../control-ui/index.html');
const controlUiHtml = readFileSync(controlUiPath, 'utf8');

controlRouter.get('/', (_req: Request, res: Response) => {
  res.type('html').send(controlUiHtml);
});

const replyState = (res: Response): void => {
  res.json(getState());
};

controlRouter.post('/scenario', (req: Request, res: Response) => {
  const name = req.body?.name;
  if (typeof name !== 'string' || !hasScenario(name)) {
    sendServiceError(res, 'invalid_argument', `unknown scenario: ${String(name)}`);
    return;
  }
  resetState(name);
  replyState(res);
});

controlRouter.post('/patch', (req: Request, res: Response) => {
  patchState((req.body ?? {}) as PatchInput);
  replyState(res);
});

controlRouter.post('/errors', (req: Request, res: Response) => {
  const body = (req.body ?? {}) as { method?: string; code?: InjectableError | null };
  if (typeof body.method !== 'string') {
    sendServiceError(res, 'invalid_argument', 'method is required');
    return;
  }
  const state = getState();
  if (body.code === null || body.code === undefined) {
    delete state.forcedErrors[body.method];
  } else {
    state.forcedErrors[body.method] = body.code;
  }
  replyState(res);
});

controlRouter.post('/tick', (_req: Request, res: Response) => {
  tick();
  replyState(res);
});

controlRouter.post('/state', (req: Request, res: Response) => {
  setState((req.body ?? {}) as MockState);
  replyState(res);
});

controlRouter.post('/reset', (_req: Request, res: Response) => {
  resetState();
  replyState(res);
});
