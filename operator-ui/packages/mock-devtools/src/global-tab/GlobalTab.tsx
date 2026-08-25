import { ControlField } from '../control-field/ControlField';
import { ScenarioList } from '../scenario-list/ScenarioList';
import { StatePatcher } from '../state-patcher/StatePatcher';
import type { PanelConfig, ScenarioCatalogEntry, ScenarioStore } from '../types';
import { useMockRevision } from '../use-mock-revision/useMockRevision';
import { VerbErrorList } from '../verb-error-list/VerbErrorList';
import styles from './GlobalTab.module.css';

export interface GlobalTabProps<W> {
  store: ScenarioStore<W>;
  catalog: readonly ScenarioCatalogEntry[];
  config: PanelConfig;
  activeScenario: string;
  onSelectScenario: (name: string) => void;
}

export const GlobalTab = <W,>({
  store,
  catalog,
  config,
  activeScenario,
  onSelectScenario
}: GlobalTabProps<W>) => {
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

  // Re-renders on any world change, including one made through
  // `window.__mockControl` rather than this panel.
  useMockRevision(store);

  const active = config.errors.active();
  const fields = config.controls.map((control) => ({ ...control, value: control.read() }));

  const handleError = (verb: string, code: string | null) => config.errors.set(verb, code);

  const handlePatch = (path: string, value: unknown) => config.patch(path, value);

  const handleControl = (id: string, next: string) => {
    const control = config.controls.find((candidate) => candidate.id === id);
    control?.write(next);
  };

  return (
    <div className={styles.tab}>
      <section className={styles.scenarios}>
        <h2 className={styles.scenariosHeading}>Scenarios</h2>

        <ScenarioList entries={catalog} activeName={activeScenario} onSelect={onSelectScenario} />
      </section>

      <section className={styles.section}>
        <h2 className={styles.heading}>Controls</h2>

        <div className={styles.controls}>
          {fields.map((field) => (
            // Remounting on an external change re-seeds the number field's
            // draft, which a scenario switch or a scripted patch can move.
            <ControlField
              key={`${field.id}:${field.value}`}
              name={field.id}
              label={field.label}
              kind={field.kind}
              options={field.options}
              value={field.value}
              onCommit={handleControl}
            />
          ))}
        </div>
      </section>

      <section className={styles.section}>
        <h2 className={styles.heading}>Error injection</h2>

        <VerbErrorList
          verbs={config.errors.verbs}
          codes={config.errors.codes}
          active={active}
          onChange={handleError}
        />
      </section>

      <section className={styles.section}>
        <h2 className={styles.heading}>Patch state</h2>

        <StatePatcher onPatch={handlePatch} />
      </section>
    </div>
  );
};
