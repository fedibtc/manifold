export interface NavItem {
  key: string;
  label: string;
  path: string;
}

// Setup is deliberately absent. It is a full-screen wizard SetupGate raises
// above the shell, not a destination inside it — there is no /setup route to
// link to, and once setup is done the way back to the configuration is
// Settings. Mirrors the fleet-manager shell.
export const NAV_ITEMS: NavItem[] = [
  { key: 'overview', label: 'Overview', path: '/' },
  { key: 'funds', label: 'Funds', path: '/funds' },
  { key: 'advertisement', label: 'Advertisement', path: '/advertisement' },
  { key: 'allocations', label: 'Allocations', path: '/allocations' },
  { key: 'settings', label: 'Settings', path: '/settings' }
];
