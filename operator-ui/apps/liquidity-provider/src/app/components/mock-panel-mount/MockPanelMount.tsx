import { useQueryClient } from '@tanstack/react-query';
import { lazy, Suspense, useEffect } from 'react';
import { useLocation } from 'react-router-dom';

// `import.meta.env.DEV` is a build-time constant, so in a production build
// this condition folds to `false` and the branch below — including the
// `@/mocks/*` imports it reaches, and the mock world data they carry — is dead
// code that never reaches Rollup's chunk graph. A runtime guard inside the
// component (checked only when rendering) is not enough: `@/mocks/store` runs
// to completion once, but by then it has already been bundled. See
// `@/app/index.tsx`, which gates `@/mocks/start` the same way, for the pattern
// this mirrors.
const mocksEnabled = import.meta.env.DEV && import.meta.env.VITE_MOCKS !== 'off';

interface BoundMockPanelProps {
  pathname: string;
}

// The pathname is resolved to a route key *inside* the lazy factory, so
// `@/mocks/routes` stays out of the production module graph. The mount below
// only knows about `useLocation`, which the app already ships.
const MockPanel = mocksEnabled
  ? lazy(async () => {
      const [
        { MockPanel: Panel },
        { mockStore },
        { scenarioCatalog },
        { panelConfig },
        { verbLog },
        { routeToKey }
      ] = await Promise.all([
        import('@operator-ui/mock-devtools/panel'),
        import('@/mocks/store'),
        import('@/mocks/scenarios'),
        import('@/mocks/panel-config'),
        import('@/mocks/verb-log'),
        import('@/mocks/routes')
      ]);

      const BoundMockPanel = ({ pathname }: BoundMockPanelProps) => (
        <Panel
          store={mockStore}
          catalog={scenarioCatalog}
          config={panelConfig}
          verbLog={verbLog}
          routeKey={routeToKey(pathname)}
          appName="FLIP"
        />
      );
      return { default: BoundMockPanel };
    })
  : null;

export const MockPanelMount = () => {
  const queryClient = useQueryClient();
  const { pathname } = useLocation();

  // Subscribe to the store rather than the panel's button, so a scenario or
  // control set through `window.__mockControl` — the surface Playwright drives
  // — refreshes the screen too. Spec §4.1.
  useEffect(() => {
    if (!mocksEnabled) return;

    let cancelled = false;
    let unsubscribe: () => void = () => undefined;

    void import('@/mocks/store').then(({ mockStore }) => {
      if (cancelled) return;
      unsubscribe = mockStore.subscribe(() => {
        void queryClient.invalidateQueries();
      });
    });

    return () => {
      cancelled = true;
      unsubscribe();
    };
  }, [queryClient]);

  if (!MockPanel) return null;

  return (
    <Suspense fallback={null}>
      <MockPanel pathname={pathname} />
    </Suspense>
  );
};
