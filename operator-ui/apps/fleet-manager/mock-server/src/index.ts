import express from 'express';
import { hasScenario } from '../../src/mocks/scenarios';
import { resetState } from '../../src/mocks/state';
import { applyLatency, noStore, requireSession } from './middleware';
import { adminRouter } from './routes/admin';
import { authRouter } from './routes/auth';
import { controlRouter } from './routes/control';

// FMan operator API mock — a puppet, not a second daemon.
// Canonical verb contract: crates/fman/specs/SPEC-admin-socket.md and
// crates/fman/core/src/admin.rs. The HTTP adapter it imitates is
// crates/fman/core/src/admin_http.rs (crates/fman/specs/SPEC-operator-http.md).
const PORT = Number(process.env.PORT ?? 8788);

const initialScenario = process.env.MOCK_SCENARIO ?? 'fresh-fleet';
if (hasScenario(initialScenario)) {
  resetState(initialScenario);
} else {
  console.warn(`unknown MOCK_SCENARIO "${initialScenario}", falling back to fresh-fleet`);
  resetState('fresh-fleet');
}

const app = express();
// strict:false so top-level `null` bodies parse — unit-variant requests (e.g. "ListSeats")
// serialize as a bare JSON string, not an object.
app.use(express.json({ strict: false }));
app.use(noStore);

app.use('/api/auth', authRouter);
app.use('/api/admin', requireSession, applyLatency, adminRouter);
app.use('/__control', controlRouter);

app.listen(PORT, () => {
  console.log(`fman-mock-server listening on :${PORT} (scenario: ${initialScenario})`);
});
