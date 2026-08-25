import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { type Request, type Response, Router, type Router as RouterType } from 'express';
import { hasScenario, scenarioCatalog, scenarioNames } from '../../../src/mocks/scenarios';
import {
  getState,
  type MockState,
  type PatchInput,
  patchState,
  resetState,
  setState
} from '../../../src/mocks/state';
import { clearSession } from '../middleware';
import { adminMethods } from './admin';

// Dev-only control surface. Open (no auth). Never shipped to production.
export const controlRouter: RouterType = Router();

const controlUiPath = join(dirname(fileURLToPath(import.meta.url)), '../control-ui/index.html');
const controlUiHtml = readFileSync(controlUiPath, 'utf8');

controlRouter.get('/', (_req: Request, res: Response) => {
  res.type('html').send(controlUiHtml);
});

// `scenarios` carries the documentation the control panel renders;
// `scenarioNames` stays for callers that only need the list.
controlRouter.get('/scenarios', (_req: Request, res: Response) => {
  res.json({ scenarioNames, scenarios: scenarioCatalog });
});

controlRouter.get('/methods', (_req: Request, res: Response) => {
  res.json({ methods: adminMethods });
});

const replyState = (res: Response): void => {
  res.json(getState());
};

controlRouter.post('/scenario', (req: Request, res: Response) => {
  const name = req.body?.name;
  if (typeof name !== 'string' || !hasScenario(name)) {
    res.status(400).json({ error: `unknown scenario: ${String(name)}` });
    return;
  }
  resetState(name);
  clearSession();
  replyState(res);
});

controlRouter.post('/patch', (req: Request, res: Response) => {
  patchState((req.body ?? {}) as PatchInput);
  replyState(res);
});

controlRouter.post('/errors', (req: Request, res: Response) => {
  const body = (req.body ?? {}) as { method?: string; message?: string | null };
  if (typeof body.method !== 'string') {
    res.status(400).json({ error: 'method is required' });
    return;
  }
  const state = getState();
  if (!body.message) {
    delete state.forcedErrors[body.method];
  } else {
    state.forcedErrors[body.method] = body.message;
  }
  replyState(res);
});

controlRouter.post('/state', (req: Request, res: Response) => {
  setState((req.body ?? {}) as MockState);
  replyState(res);
});

controlRouter.post('/reset', (_req: Request, res: Response) => {
  resetState();
  clearSession();
  replyState(res);
});
