import { createScenarioStore, type WorldSource } from '@operator-ui/mock-devtools';
import { hasScenario, scenario } from '@/mocks/scenarios';
import type { MockState } from '@/mocks/state';

// FLIP's bearer token lives in the app's in-memory tokenStore, not in
// MockState, so there is nothing to carryOver on a scenario switch.
const source: WorldSource<MockState> = {
  appKey: 'flip',
  defaultScenario: 'setup-fresh',
  has: hasScenario,
  build: scenario
};

export const mockStore = createScenarioStore(source);
