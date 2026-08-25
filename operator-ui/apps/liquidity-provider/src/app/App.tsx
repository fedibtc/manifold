import '@/app/app.css';
import { QueryClientProvider } from '@tanstack/react-query';
import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { createBrowserRouter, RouterProvider } from 'react-router-dom';
import { AppShell } from '@/app/components/app-shell/AppShell';
import { BootGate } from '@/app/components/boot-gate/BootGate';
import { RootLayout } from '@/app/components/root-layout/RootLayout';
import { SetupGate } from '@/app/components/setup-gate/SetupGate';
import { AdvertisementPage } from '@/pages/advertisement/AdvertisementPage';
import { AllocationsPage } from '@/pages/allocations/AllocationsPage';
import { FundsPage } from '@/pages/funds/FundsPage';
import { OverviewPage } from '@/pages/overview/OverviewPage';
import { SettingsPage } from '@/pages/settings/SettingsPage';
import { queryClient } from '@/shared/api/queryClient';

// Gate order: BootGate (daemon reachable, session authenticated) → SetupGate
// (setup reached `ready`) → AppShell. Setup is not a route: it is a
// full-screen wizard SetupGate renders in place of the whole shell, so there
// is no URL that puts the sidebar and the wizard on screen together and no
// nav row that can reach it. The operator's route survives underneath and
// resumes once the gate lifts.
//
// RootLayout sits above all three gates and mounts the dev mock panel: a
// scenario switch can latch SetupGate into the setup wizard, swapping
// AppShell out of the tree, so the panel has to live somewhere that survives
// that swap rather than inside AppShell itself.
const router = createBrowserRouter([
  {
    element: <RootLayout />,
    children: [
      {
        element: <BootGate />,
        children: [
          {
            element: <SetupGate />,
            children: [
              {
                element: <AppShell />,
                children: [
                  { index: true, element: <OverviewPage /> },
                  { path: 'funds', element: <FundsPage /> },
                  { path: 'allocations', element: <AllocationsPage /> },
                  { path: 'advertisement', element: <AdvertisementPage /> },
                  { path: 'settings', element: <SettingsPage /> }
                ]
              }
            ]
          }
        ]
      }
    ]
  }
]);

const root = document.getElementById('root');
if (!root) throw new Error('missing #root element');

const render = () => {
  createRoot(root).render(
    <StrictMode>
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
      </QueryClientProvider>
    </StrictMode>
  );
};

// The worker must be listening before the first query fires, so rendering is
// deferred until it is. The dynamic import behind a statically-analysable guard
// is what keeps the whole mock subtree out of production bundles.
if (import.meta.env.DEV && import.meta.env.VITE_MOCKS !== 'off') {
  const { startMocks } = await import('@/mocks/start');
  await startMocks();
}

render();
