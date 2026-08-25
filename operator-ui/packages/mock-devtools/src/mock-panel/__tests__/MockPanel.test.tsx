import { act, fireEvent, render, screen } from '@testing-library/react';
import { vi } from 'vitest';
import { createScenarioStore } from '../../scenario-store';
import type { PanelConfig, ScenarioStore, StorageAdapter, VerbLog, WorldSource } from '../../types';
import { createVerbLog } from '../../verb-log';
import { MockPanel } from '../MockPanel';

interface TestWorld {
  seats: string[];
}

const source: WorldSource<TestWorld> = {
  appKey: 'test',
  defaultScenario: 'empty',
  has: (name) => name === 'empty' || name === 'populated',
  build: (name) => ({ seats: name === 'populated' ? ['seat-1'] : [] })
};

const memoryStorage = (): StorageAdapter => {
  const contents: Record<string, string> = {};
  return {
    load: (key) => contents[key] ?? null,
    save: (key, value) => {
      contents[key] = value;
    }
  };
};

const catalog = [
  { name: 'empty', desc: 'Nothing sold yet.', affects: ['overview'] },
  { name: 'populated', desc: 'One running seat.', affects: ['seats'] }
];

// Mirrors the real apps: every writer persists and notifies (see PanelConfig),
// and `active()` hands back a snapshot rather than the world's own mutable map.
const testConfig = (store: ScenarioStore<TestWorld>) => {
  const knobs: Record<string, string> = { latencyMs: '0', authMode: 'password' };
  const forced: Record<string, string> = {};
  const patches: { path: string; value: unknown }[] = [];

  const config: PanelConfig = {
    controls: [
      {
        id: 'latencyMs',
        label: 'Latency (ms)',
        kind: 'number',
        read: () => knobs.latencyMs,
        write: (value) => {
          knobs.latencyMs = value;
          store.notify();
        }
      },
      {
        id: 'authMode',
        label: 'Auth mode',
        kind: 'select',
        options: ['password', 'trusted_proxy'],
        read: () => knobs.authMode,
        write: (value) => {
          knobs.authMode = value;
          store.notify();
        }
      }
    ],
    errors: {
      verbs: ['ListSeats', 'GuardianFees'],
      codes: ['unknown seat', 'internal error'],
      active: () => ({ ...forced }),
      set: (verb, code) => {
        if (code === null) delete forced[verb];
        else forced[verb] = code;
        store.notify();
      }
    },
    patch: (path, value) => {
      patches.push({ path, value });
      store.notify();
    }
  };

  return { config, knobs, forced, patches };
};

const renderPanel = (routeKey: string | null = 'seats') => {
  const store = createScenarioStore(source, memoryStorage());
  const verbLog = createVerbLog(() => routeKey ?? 'unrouted');
  const { config, knobs, forced, patches } = testConfig(store);

  render(
    <MockPanel
      store={store}
      catalog={catalog}
      config={config}
      verbLog={verbLog}
      routeKey={routeKey}
      appName="Test App"
    />
  );

  return { store, verbLog, knobs, forced, patches };
};

const open = () => fireEvent.click(screen.getByRole('button', { name: /mock controls/i }));

const showGlobal = () => fireEvent.click(screen.getByRole('tab', { name: 'All app' }));

// The mock serves verbs outside React's knowledge, so the notification that
// reaches the panel has to be flushed the way a real fetch resolution would be.
const serve = (verbLog: VerbLog, verb: string) => act(() => verbLog.record(verb));

it('should stay collapsed until opened', () => {
  renderPanel();

  expect(screen.queryByRole('tab')).not.toBeInTheDocument();
});

it('should name the current route on the per-page tab', () => {
  renderPanel('seat-detail');

  open();

  expect(screen.getByRole('tab', { name: 'This screen: Seat detail' })).toBeInTheDocument();
});

it('should open on the per-page tab when a scenario affects this route', () => {
  renderPanel('seats');

  open();

  expect(screen.getByRole('tab', { name: /this screen/i })).toHaveAttribute(
    'aria-selected',
    'true'
  );
});

it('should open on the app-wide tab when no scenario affects this route', () => {
  renderPanel('backup');

  open();

  expect(screen.getByRole('tab', { name: 'All app' })).toHaveAttribute('aria-selected', 'true');
});

it('should show only the scenarios affecting this route on the per-page tab', () => {
  renderPanel('seats');

  open();

  expect(screen.getByText('One running seat.')).toBeInTheDocument();
  expect(screen.queryByText('Nothing sold yet.')).not.toBeInTheDocument();
});

it('should say how many scenarios affect this route', () => {
  renderPanel('seats');

  open();

  expect(screen.getByText(/1 of 2 affect this page/i)).toBeInTheDocument();
});

it('should list every scenario on the global tab', () => {
  renderPanel('seats');
  open();

  showGlobal();

  expect(screen.getByText('Nothing sold yet.')).toBeInTheDocument();
  expect(screen.getByText('One running seat.')).toBeInTheDocument();
});

it('should load the scenario the operator toggles on', () => {
  const { store } = renderPanel('seats');
  open();

  fireEvent.click(screen.getByRole('switch', { name: /populated/i }));

  expect(store.getScenario()).toBe('populated');
});

it('should show the persisted scenario as the one switched on', () => {
  renderPanel('seats');
  open();

  showGlobal();

  expect(screen.getByRole('switch', { name: /empty/i })).toHaveAttribute('aria-checked', 'true');
  expect(screen.getByRole('switch', { name: /populated/i })).toHaveAttribute(
    'aria-checked',
    'false'
  );
});

it('should hide reset while the default scenario is active', () => {
  renderPanel('seats');

  open();

  expect(screen.queryByRole('button', { name: /reset mocks/i })).not.toBeInTheDocument();
});

it('should offer reset once a non-default scenario is active', () => {
  renderPanel('seats');
  open();

  fireEvent.click(screen.getByRole('switch', { name: /populated/i }));

  expect(screen.getByRole('button', { name: /reset mocks/i })).toBeInTheDocument();
});

it('should return to the default scenario on reset', () => {
  const { store } = renderPanel('seats');
  open();
  fireEvent.click(screen.getByRole('switch', { name: /populated/i }));

  fireEvent.click(screen.getByRole('button', { name: /reset mocks/i }));

  expect(store.getScenario()).toBe('empty');
});

it('should say it is listening while no verb has been served on this route', () => {
  renderPanel('seats');

  open();

  expect(screen.getByText(/listening for this page's calls/i)).toBeInTheDocument();
});

it('should list a verb once the mock has served it on this route', () => {
  const { verbLog } = renderPanel('seats');
  open();

  serve(verbLog, 'ListSeats');

  expect(screen.getByLabelText('ListSeats')).toBeInTheDocument();
});

it('should not list a verb served while another route was showing', () => {
  const store = createScenarioStore(source, memoryStorage());
  let showing = 'wallet';
  const verbLog = createVerbLog(() => showing);
  const { config } = testConfig(store);
  render(
    <MockPanel
      store={store}
      catalog={catalog}
      config={config}
      verbLog={verbLog}
      routeKey="seats"
      appName="Test App"
    />
  );
  open();

  serve(verbLog, 'ListPaymentFederations');
  showing = 'seats';
  serve(verbLog, 'ListSeats');

  expect(screen.getByLabelText('ListSeats')).toBeInTheDocument();
  expect(screen.queryByLabelText('ListPaymentFederations')).not.toBeInTheDocument();
});

it('should stop listing verbs cleared for this route', () => {
  const { verbLog } = renderPanel('seats');
  open();
  serve(verbLog, 'ListSeats');

  fireEvent.click(screen.getByRole('button', { name: 'clear' }));

  expect(screen.getByText(/listening for this page's calls/i)).toBeInTheDocument();
});

it('should inject an error on a verb chosen from the per-page tab', () => {
  const { verbLog, forced } = renderPanel('seats');
  open();
  serve(verbLog, 'ListSeats');

  fireEvent.change(screen.getByLabelText('ListSeats'), { target: { value: 'unknown seat' } });

  expect(forced).toEqual({ ListSeats: 'unknown seat' });
});

it('should offer every dispatchable verb on the global tab', () => {
  renderPanel('seats');
  open();

  showGlobal();

  expect(screen.getByLabelText('ListSeats')).toBeInTheDocument();
  expect(screen.getByLabelText('GuardianFees')).toBeInTheDocument();
});

it('should clear an injected error when the verb returns to no error', () => {
  const { forced } = renderPanel('seats');
  open();
  showGlobal();
  fireEvent.change(screen.getByLabelText('ListSeats'), { target: { value: 'internal error' } });

  fireEvent.change(screen.getByLabelText('ListSeats'), { target: { value: '' } });

  expect(forced).toEqual({});
});

it('should commit a select control as soon as it changes', () => {
  const { knobs } = renderPanel('seats');
  open();
  showGlobal();

  fireEvent.change(screen.getByLabelText('Auth mode'), { target: { value: 'trusted_proxy' } });

  expect(knobs.authMode).toBe('trusted_proxy');
});

it('should hold a number control until it is applied', () => {
  const { knobs } = renderPanel('seats');
  open();
  showGlobal();

  fireEvent.change(screen.getByLabelText('Latency (ms)'), { target: { value: '250' } });

  expect(knobs.latencyMs).toBe('0');
});

it('should commit a number control on apply', () => {
  const { knobs } = renderPanel('seats');
  open();
  showGlobal();
  fireEvent.change(screen.getByLabelText('Latency (ms)'), { target: { value: '250' } });

  fireEvent.click(screen.getByRole('button', { name: 'Apply Latency (ms)' }));

  expect(knobs.latencyMs).toBe('250');
});

it('should patch the world at a dotted path with a parsed value', () => {
  const { patches } = renderPanel('seats');
  open();
  showGlobal();
  fireEvent.change(screen.getByLabelText('Path'), { target: { value: 'seats.0.health' } });
  fireEvent.change(screen.getByLabelText('Value'), { target: { value: '"unavailable"' } });

  fireEvent.click(screen.getByRole('button', { name: 'Apply patch' }));

  expect(patches).toEqual([{ path: 'seats.0.health', value: 'unavailable' }]);
});

it('should refuse a patch value that is not JSON', () => {
  const { patches } = renderPanel('seats');
  open();
  showGlobal();
  fireEvent.change(screen.getByLabelText('Path'), { target: { value: 'seats.0.health' } });
  fireEvent.change(screen.getByLabelText('Value'), { target: { value: 'unavailable' } });

  fireEvent.click(screen.getByRole('button', { name: 'Apply patch' }));

  expect(patches).toEqual([]);
  expect(screen.getByText('value must be JSON')).toBeInTheDocument();
});

// Regression: `forcedErrors` lives in the world and is mutated in place, so
// neither the injection nor the control write changes any prop. Without the
// store revision as a real render dependency — and a snapshot from `active()` —
// the panel keeps showing the pre-injection value while the mock has already
// changed behaviour.
it('should show an injected error on the verb it was set for', () => {
  renderPanel('seats');
  open();
  showGlobal();

  fireEvent.change(screen.getByLabelText('ListSeats'), { target: { value: 'unknown seat' } });

  expect(screen.getByLabelText('ListSeats')).toHaveValue('unknown seat');
});

it('should show an injection made outside the panel', () => {
  const { store, forced } = renderPanel('seats');
  open();
  showGlobal();

  act(() => {
    forced.GuardianFees = 'internal error';
    store.notify();
  });

  expect(screen.getByLabelText('GuardianFees')).toHaveValue('internal error');
});

it('should show a committed control value', () => {
  renderPanel('seats');
  open();
  showGlobal();
  fireEvent.change(screen.getByLabelText('Latency (ms)'), { target: { value: '250' } });

  fireEvent.click(screen.getByRole('button', { name: 'Apply Latency (ms)' }));

  expect(screen.getByLabelText('Latency (ms)')).toHaveValue(250);
});

it('should show a control value changed outside the panel', () => {
  const { store, knobs } = renderPanel('seats');
  open();
  showGlobal();

  act(() => {
    knobs.authMode = 'trusted_proxy';
    store.notify();
  });

  expect(screen.getByLabelText('Auth mode')).toHaveValue('trusted_proxy');
});

it('should name the app and the visible screen in the header', () => {
  renderPanel('seat-detail');

  open();

  expect(screen.getByText('Test App · Seat detail')).toBeInTheDocument();
});

it('should always name the active scenario in the header', () => {
  renderPanel('seats');

  open();

  const scenarioLine = screen.getByText(/^Scenario:/);
  expect(scenarioLine).toHaveTextContent('Scenario: empty');
});

it('should show only app-wide controls when no route claims the surface', () => {
  renderPanel(null);

  open();

  expect(screen.queryByRole('tab')).not.toBeInTheDocument();
  expect(screen.getByText(/showing app-wide controls/i)).toBeInTheDocument();
  expect(screen.getByText('Test App · All app')).toBeInTheDocument();
  expect(screen.getByText('Nothing sold yet.')).toBeInTheDocument();
});

it('should count injected errors in the header', () => {
  renderPanel('seats');
  open();
  showGlobal();

  fireEvent.change(screen.getByLabelText('ListSeats'), { target: { value: 'internal error' } });
  fireEvent.change(screen.getByLabelText('GuardianFees'), { target: { value: 'unknown seat' } });

  expect(screen.getByText('2 injected errors')).toBeInTheDocument();
});

it('should clear every injected error from the header', () => {
  const { forced } = renderPanel('seats');
  open();
  showGlobal();
  fireEvent.change(screen.getByLabelText('ListSeats'), { target: { value: 'internal error' } });
  fireEvent.change(screen.getByLabelText('GuardianFees'), { target: { value: 'unknown seat' } });

  fireEvent.click(screen.getByRole('button', { name: 'Clear errors' }));

  expect(forced).toEqual({});
  expect(screen.queryByText(/injected error/)).not.toBeInTheDocument();
});

// Regression: reset used to key off the scenario name alone, so overrides made
// on the default scenario — the common case — left no visible way back.
it('should offer reset once an override dirties the default scenario', () => {
  renderPanel('seats');
  open();
  showGlobal();

  fireEvent.change(screen.getByLabelText('ListSeats'), { target: { value: 'internal error' } });

  expect(screen.getByRole('button', { name: /reset mocks/i })).toBeInTheDocument();
});

it('should hide reset again once reset returns the world to its default', () => {
  renderPanel('seats');
  open();
  showGlobal();
  fireEvent.change(screen.getByLabelText('ListSeats'), { target: { value: 'internal error' } });

  fireEvent.click(screen.getByRole('button', { name: /reset mocks/i }));

  expect(screen.queryByRole('button', { name: /reset mocks/i })).not.toBeInTheDocument();
});

it('should close on Escape and return focus to the launcher', () => {
  renderPanel('seats');
  open();

  fireEvent.keyDown(screen.getByRole('tablist'), { key: 'Escape' });

  expect(screen.queryByRole('tab')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: /mock controls/i })).toHaveFocus();
});

it('should close from the header close action', () => {
  renderPanel('seats');
  open();

  fireEvent.click(screen.getByRole('button', { name: 'Close mock panel' }));

  expect(screen.queryByRole('tab')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: /mock controls/i })).toHaveFocus();
});

it('should move focus onto the selected tab when the panel opens', () => {
  renderPanel('seats');

  open();

  expect(screen.getByRole('tab', { name: /this screen/i })).toHaveFocus();
});

it('should move between tabs with arrow keys', () => {
  renderPanel('seats');
  open();

  fireEvent.keyDown(screen.getByRole('tablist'), { key: 'ArrowRight' });

  expect(screen.getByRole('tab', { name: 'All app' })).toHaveAttribute('aria-selected', 'true');
  expect(screen.getByRole('tab', { name: 'All app' })).toHaveFocus();
});

it('should copy the exported debug state to the clipboard', async () => {
  const writeText = vi.fn<(text: string) => Promise<void>>().mockResolvedValue(undefined);
  Object.defineProperty(navigator, 'clipboard', {
    value: { writeText },
    configurable: true
  });

  try {
    const { store } = renderPanel('seats');
    open();

    fireEvent.click(screen.getByRole('button', { name: 'Copy debug state' }));

    await screen.findByRole('button', { name: 'Copied' });
    expect(writeText).toHaveBeenCalledWith(store.exportState());
  } finally {
    delete (navigator as { clipboard?: unknown }).clipboard;
  }
});

it('should say so when the clipboard is unavailable', async () => {
  // jsdom has no `navigator.clipboard` at all, which is exactly the failure
  // being asserted: the button must degrade to a visible "failed", not a
  // silent no-op the developer pastes air from.
  renderPanel('seats');
  open();

  fireEvent.click(screen.getByRole('button', { name: 'Copy debug state' }));

  expect(await screen.findByRole('button', { name: 'Copy failed' })).toBeInTheDocument();
});
