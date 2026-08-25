import { type KeyboardEvent, useEffect, useId, useRef, useState } from 'react';
import { formatRouteKey, scenariosFor } from '../catalog';
import { CopyStateButton } from '../copy-state-button/CopyStateButton';
import { GlobalTab } from '../global-tab/GlobalTab';
import { PageTab } from '../page-tab/PageTab';
import { PanelHeader } from '../panel-header/PanelHeader';
import type { PanelConfig, RouteKey, ScenarioCatalogEntry, ScenarioStore, VerbLog } from '../types';
import { useScenario } from '../use-scenario/useScenario';
import styles from './MockPanel.module.css';

type PanelTab = 'page' | 'global';

export interface MockPanelProps<W> {
  store: ScenarioStore<W>;
  catalog: readonly ScenarioCatalogEntry[];
  config: PanelConfig;
  verbLog: VerbLog;
  /** Null when the visible surface has no recognised route — a gate or an
   *  unrouted path. The panel then shows only app-wide controls rather than
   *  guessing a screen. */
  routeKey: RouteKey | null;
  /** Named in the header so the developer always knows which app's mock world
   *  they are about to change. */
  appName: string;
}

export const MockPanel = <W,>({
  store,
  catalog,
  config,
  verbLog,
  routeKey,
  appName
}: MockPanelProps<W>) => {
  const { scenario, isDirty, setScenario, reset } = useScenario(store);
  const [isOpen, setIsOpen] = useState(false);
  const [tab, setTab] = useState<PanelTab>('page');
  const launcherRef = useRef<HTMLButtonElement>(null);
  const pageTabRef = useRef<HTMLButtonElement>(null);
  const globalTabRef = useRef<HTMLButtonElement>(null);
  const bodyRef = useRef<HTMLDivElement>(null);
  const pageTabId = useId();
  const globalTabId = useId();
  const panelId = useId();

  const pageEntries = routeKey === null ? [] : scenariosFor(catalog, routeKey);

  // An unrecognised surface has no page tab, so a page selection cannot
  // survive navigating onto one. Adjusted during render (guarded) per React's
  // state-adjustment idiom — the compiler rejects a bare setState-in-effect.
  if (routeKey === null && tab === 'page') setTab('global');

  const handleToggle = () => {
    if (isOpen) {
      setIsOpen(false);
      return;
    }
    // Open on the page tab only when it has something to say; otherwise the
    // app-wide view is what stops the panel from looking empty.
    setTab(routeKey !== null && pageEntries.length > 0 ? 'page' : 'global');
    setIsOpen(true);
  };

  const handleClose = () => {
    setIsOpen(false);
    launcherRef.current?.focus();
  };

  const handleEscape = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key !== 'Escape' || !isOpen) return;
    handleClose();
  };

  // Arrow keys move between the two tabs, focus following selection, as a
  // tablist is expected to behave. Roving tabindex below keeps Tab itself
  // entering the list on the selected tab only.
  const handleTabsKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return;
    const next: PanelTab = tab === 'page' ? 'global' : 'page';
    setTab(next);
    const target = next === 'page' ? pageTabRef : globalTabRef;
    target.current?.focus();
  };

  const handlePageTab = () => setTab('page');
  const handleGlobalTab = () => setTab('global');

  // Focus moves into the panel when it opens; handleClose returns it to the
  // launcher. The selected tab is found in the DOM because which tab that is
  // was decided in the same commit that opened the panel.
  useEffect(() => {
    if (!isOpen) return;
    const body = bodyRef.current;
    if (!body) return;
    const selected = body.querySelector<HTMLElement>('[role="tab"][aria-selected="true"]');
    (selected ?? body).focus();
  }, [isOpen]);

  // Reset appears once the world has moved off its default in either way a
  // world can move: a non-default scenario, or hand-made overrides on any
  // scenario (`isDirty`) — previously the latter left no way back.
  const isOverridden = scenario !== store.getDefaultScenario() || isDirty;

  const hasTabs = routeKey !== null;
  const selectedTabId = tab === 'page' ? pageTabId : globalTabId;

  // One definition for both content branches below — the branches exist only so
  // the tabpanel role and label are statically paired for the a11y lint.
  const globalTabContent = (
    <GlobalTab
      store={store}
      catalog={catalog}
      config={config}
      activeScenario={scenario}
      onSelectScenario={setScenario}
    />
  );

  return (
    <aside className={styles.panel} onKeyDown={handleEscape}>
      <button
        ref={launcherRef}
        type="button"
        className={styles.launcher}
        aria-expanded={isOpen}
        aria-controls={isOpen ? panelId : undefined}
        onClick={handleToggle}
      >
        Mock controls
      </button>

      {isOpen ? (
        <div ref={bodyRef} id={panelId} className={styles.body} tabIndex={-1}>
          <PanelHeader
            store={store}
            config={config}
            appName={appName}
            routeKey={routeKey}
            scenario={scenario}
            onClose={handleClose}
          />

          {routeKey === null ? (
            <p className={styles.fallback}>
              This surface has no route of its own — showing app-wide controls.
            </p>
          ) : (
            <div className={styles.tabs} role="tablist" onKeyDown={handleTabsKeyDown}>
              <button
                ref={pageTabRef}
                type="button"
                role="tab"
                id={pageTabId}
                aria-selected={tab === 'page'}
                tabIndex={tab === 'page' ? 0 : -1}
                className={styles.tab}
                onClick={handlePageTab}
              >
                This screen: {formatRouteKey(routeKey)}
              </button>

              <button
                ref={globalTabRef}
                type="button"
                role="tab"
                id={globalTabId}
                aria-selected={tab === 'global'}
                tabIndex={tab === 'global' ? 0 : -1}
                className={styles.tab}
                onClick={handleGlobalTab}
              >
                All app
              </button>
            </div>
          )}

          {hasTabs ? (
            <div className={styles.content} role="tabpanel" aria-labelledby={selectedTabId}>
              {tab === 'page' && routeKey !== null ? (
                <PageTab
                  store={store}
                  catalog={catalog}
                  config={config}
                  verbLog={verbLog}
                  routeKey={routeKey}
                  activeScenario={scenario}
                  onSelectScenario={setScenario}
                />
              ) : (
                globalTabContent
              )}
            </div>
          ) : (
            <div className={styles.content}>{globalTabContent}</div>
          )}

          <div className={styles.footer}>
            <CopyStateButton exportState={store.exportState} />

            {isOverridden ? (
              <button type="button" className={styles.reset} onClick={reset}>
                Reset mocks
              </button>
            ) : null}
          </div>
        </div>
      ) : null}
    </aside>
  );
};
