import { QueryClientProvider } from '@tanstack/react-query';
import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { createBrowserRouter, RouterProvider } from 'react-router-dom';
import '@/app/index.css';
import { AppShell } from '@/app/components/app-shell/AppShell';
import { BootGate } from '@/app/components/boot-gate/BootGate';
import { RootLayout } from '@/app/components/root-layout/RootLayout';
import { SetupGate } from '@/app/components/setup-gate/SetupGate';
import { AuthorizationPage } from '@/pages/authorization/AuthorizationPage';
import { BackupPage } from '@/pages/backup/BackupPage';
import { BackupPhrasePage } from '@/pages/backup-phrase/BackupPhrasePage';
import { OfferPage } from '@/pages/offer/OfferPage';
import { OverviewPage } from '@/pages/overview/OverviewPage';
import { PayoutsPage } from '@/pages/payouts/PayoutsPage';
import { SeatDetailPage } from '@/pages/seat-detail/SeatDetailPage';
import { SeatsPage } from '@/pages/seats/SeatsPage';
import { WalletPage } from '@/pages/wallet/WalletPage';
import { queryClient } from '@/shared/api/queryClient';

// Gate order: BootGate (daemon reachable, session authenticated) → SetupGate
// (this host has an identity) → AppShell. Setup sits above the shell so no
// sidebar renders for a fleet that does not exist yet.
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
                  { path: 'authorization', element: <AuthorizationPage /> },
                  { path: 'seats', element: <SeatsPage /> },
                  { path: 'seats/:seatId', element: <SeatDetailPage /> },
                  { path: 'wallet', element: <WalletPage /> },
                  { path: 'payouts', element: <PayoutsPage /> },
                  { path: 'offer', element: <OfferPage /> },
                  { path: 'backup', element: <BackupPage /> },
                  { path: 'backup/phrase', element: <BackupPhrasePage /> }
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
