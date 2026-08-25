import { Outlet } from 'react-router-dom';
import { MockPanelMount } from '@/app/components/mock-panel-mount/MockPanelMount';

// Router root, above BootGate/SetupGate/AppShell. The mock panel mounts here
// rather than inside AppShell so that a scenario switch which latches
// SetupGate into the setup wizard (e.g. `not-onboarded`) swaps out AppShell
// without taking the panel with it — the developer keeps a way back out.
export const RootLayout = () => (
  <>
    <Outlet />

    <MockPanelMount />
  </>
);
