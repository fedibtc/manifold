import { scenariosFor } from '../catalog';
import { ScenarioList } from '../scenario-list/ScenarioList';
import type { PanelConfig, RouteKey, ScenarioCatalogEntry, ScenarioStore, VerbLog } from '../types';
import { useMockRevision } from '../use-mock-revision/useMockRevision';
import { useVerbLog } from '../use-verb-log/useVerbLog';
import { VerbErrorList } from '../verb-error-list/VerbErrorList';
import styles from './PageTab.module.css';

export interface PageTabProps<W> {
  store: ScenarioStore<W>;
  catalog: readonly ScenarioCatalogEntry[];
  config: PanelConfig;
  verbLog: VerbLog;
  routeKey: RouteKey;
  activeScenario: string;
  onSelectScenario: (name: string) => void;
}

export const PageTab = <W,>({
  store,
  catalog,
  config,
  verbLog,
  routeKey,
  activeScenario,
  onSelectScenario
}: PageTabProps<W>) => {
  // The mock world is mutated in place, so this component is not a pure function
  // of its props: the same `config` yields different control values and forced
  // errors from one revision to the next. The React Compiler's central
  // assumption does not hold here, and empirically it caches `config.errors
  // .active()` — a call on a stable object — across renders regardless of what
  // a `useMemo` declares as its dependencies, leaving the panel showing state
  // the mock has already moved off. This opt-out is permanent rather than a
  // TODO: the fix would be an immutable mock world, which is a larger change
  // than this panel, and memoizing a dev-only drawer buys nothing.
  'use no memo';

  useMockRevision(store);

  const active = config.errors.active();
  const verbs = useVerbLog(verbLog, routeKey);
  const entries = scenariosFor(catalog, routeKey);

  const handleError = (verb: string, code: string | null) => config.errors.set(verb, code);

  const handleClear = () => verbLog.clear(routeKey);

  return (
    <div className={styles.tab}>
      <section className={styles.scenarios}>
        <h2 className={styles.scenariosHeading}>
          Scenarios · {entries.length} of {catalog.length} affect this page
        </h2>

        {entries.length > 0 ? (
          <ScenarioList entries={entries} activeName={activeScenario} onSelect={onSelectScenario} />
        ) : (
          <p className={styles.empty}>No scenario changes what this page renders.</p>
        )}
      </section>

      <section className={styles.section}>
        <div className={styles.sectionHead}>
          <h2 className={styles.heading}>Verbs served on this page</h2>

          {verbs.length > 0 ? (
            <button type="button" className={styles.clear} onClick={handleClear}>
              clear
            </button>
          ) : null}
        </div>

        {verbs.length > 0 ? (
          <VerbErrorList
            verbs={verbs}
            codes={config.errors.codes}
            active={active}
            onChange={handleError}
          />
        ) : (
          // The list is built from traffic the mock has actually served, so it
          // is empty until this page's first fetch resolves. Saying "listening"
          // stays true however long it sits there, which a page that fetches
          // nothing needs it to.
          <p className={styles.listening}>
            <span className={styles.spinner} />
            Listening for this page's calls…
          </p>
        )}
      </section>
    </div>
  );
};
