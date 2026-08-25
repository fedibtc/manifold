import { createScenarioStore, type WorldSource } from '@operator-ui/mock-devtools';
import { hasScenario, scenario } from '@/mocks/scenarios';
import type { MockState } from '@/mocks/state';

const source: WorldSource<MockState> = {
  appKey: 'fman',
  defaultScenario: 'fresh-fleet',
  has: hasScenario,
  build: scenario,
  // Being logged out on every scenario switch is friction the Express panel
  // never had, because it lived on its own page. The session describes the
  // mock's auth, not the state of the fleet. Spec §4.1.
  carryOver: (previous, next) => {
    next.sessionActive = previous.sessionActive;
  }
};

export const mockStore = createScenarioStore(source);
