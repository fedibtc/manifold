export interface NavItem {
  key: string;
  label: string;
  path: string;
}

export const NAV_ITEMS: NavItem[] = [
  { key: 'overview', label: 'Overview', path: '/' },
  { key: 'authorization', label: 'Authorization', path: '/authorization' },
  { key: 'seats', label: 'Seats', path: '/seats' },
  { key: 'wallet', label: 'Wallet', path: '/wallet' },
  { key: 'payouts', label: 'Payouts', path: '/payouts' },
  { key: 'backup', label: 'Backup', path: '/backup' }
];
