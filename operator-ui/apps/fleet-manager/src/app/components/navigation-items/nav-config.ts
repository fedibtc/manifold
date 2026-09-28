export interface NavItem {
  key: string;
  label: string;
  path: string;
}

export const NAV_ITEMS: NavItem[] = [
  { key: 'overview', label: 'Overview', path: '/' },
  { key: 'health', label: 'Health', path: '/health' },
  { key: 'authorization', label: 'Authorization', path: '/authorization' },
  { key: 'seats', label: 'Seats', path: '/seats' },
  { key: 'payouts', label: 'Payouts', path: '/payouts' },
  { key: 'backup', label: 'Backup', path: '/backup' }
];
