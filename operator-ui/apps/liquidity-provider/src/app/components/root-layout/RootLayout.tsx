import { Outlet } from 'react-router-dom';
import { MockPanelMount } from '@/app/components/mock-panel-mount/MockPanelMount';

// Router root, above BootGate/SetupGate/AppShell. The mock panel mounts here
// rather than inside AppShell because both gates render *instead of* their
// Outlet, and the panel is what sends them there: its boot-mode control puts
// BootGate on the restore console, and a scenario switch latches SetupGate into
// the setup wizard. Mounted below either gate, the panel would delete itself
// the first time it was used and leave no way back out.
export const RootLayout = () => (
  <>
    <Outlet />

    <MockPanelMount />
  </>
);
