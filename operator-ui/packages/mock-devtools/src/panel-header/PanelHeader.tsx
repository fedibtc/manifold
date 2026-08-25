import { formatRouteKey } from '../catalog';
import type { PanelConfig, RouteKey, ScenarioStore } from '../types';
import { useMockRevision } from '../use-mock-revision/useMockRevision';
import styles from './PanelHeader.module.css';

export interface PanelHeaderProps<W> {
  store: ScenarioStore<W>;
  config: PanelConfig;
  appName: string;
  routeKey: RouteKey | null;
  scenario: string;
  onClose: () => void;
}

export const PanelHeader = <W,>({
  store,
  config,
  appName,
  routeKey,
  scenario,
  onClose
}: PanelHeaderProps<W>) => {
  // `config.errors.active()` reads the mutable mock world, so this component is
  // not a pure function of its props — the same opt-out, for the same reason,
  // as PageTab and GlobalTab. See the comment there.
  'use no memo';

  useMockRevision(store);

  const surface = routeKey === null ? 'All app' : formatRouteKey(routeKey);
  const errorVerbs = Object.keys(config.errors.active());
  const errorNoun = errorVerbs.length === 1 ? 'error' : 'errors';

  const handleClearErrors = () => {
    for (const verb of errorVerbs) config.errors.set(verb, null);
  };

  return (
    <header className={styles.header}>
      <div className={styles.titleRow}>
        <p className={styles.title}>
          {appName} · {surface}
        </p>

        <button
          type="button"
          className={styles.close}
          aria-label="Close mock panel"
          onClick={onClose}
        >
          ✕
        </button>
      </div>

      <p className={styles.scenario}>
        Scenario: <span className={styles.scenarioName}>{scenario}</span>
      </p>

      {errorVerbs.length > 0 ? (
        <div className={styles.errors}>
          <span>
            {errorVerbs.length} injected {errorNoun}
          </span>

          <button type="button" className={styles.clearErrors} onClick={handleClearErrors}>
            Clear errors
          </button>
        </div>
      ) : null}
    </header>
  );
};
