import express from 'express';
import { hasScenario } from '../../src/mocks/scenarios';
import { resetState } from '../../src/mocks/state';
import { applyLatency, bearerAuth } from './middleware';
import { adminRouter } from './routes/admin';
import { controlRouter } from './routes/control';
import { healthRouter } from './routes/health';

// FLIP Admin API mock — a puppet, not a second daemon.
// Contract: docs/liquidity-manager/liquidity-manager-admin-api.md
const PORT = Number(process.env.PORT ?? 8787);

const initialScenario = process.env.MOCK_SCENARIO ?? 'setup-fresh';
if (hasScenario(initialScenario)) {
  resetState(initialScenario);
} else {
  console.warn(`unknown MOCK_SCENARIO "${initialScenario}", falling back to setup-fresh`);
  resetState('setup-fresh');
}

const app = express();
// strict:false so top-level `null` bodies parse — unit-struct requests
// (e.g. get_setup_state, get_advertisement_state) serialize as JSON `null`.
app.use(express.json({ strict: false }));

app.use(healthRouter);
app.use('/admin/v1', bearerAuth, applyLatency, adminRouter);
app.use('/__control', controlRouter);

app.listen(PORT, () => {
  console.log(`flip-mock-server listening on :${PORT} (scenario: ${initialScenario})`);
});
