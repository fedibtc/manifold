export interface NavItem {
  key: string;
  label: string;
  path: string;
}

export const NAV_ITEMS: NavItem[] = [
  { key: 'overview', label: 'Overview', path: '/' },
  { key: 'authorization', label: 'Guardian badge', path: '/authorization' },
  { key: 'seats', label: 'Seats', path: '/seats' },
  { key: 'guardian-codes', label: 'Guardian codes', path: '/guardian-codes' },
  { key: 'payouts', label: 'Payouts', path: '/payouts' },
  { key: 'backup', label: 'Backup', path: '/backup' },
  { key: 'health', label: 'Health', path: '/health' },
  { key: 'support', label: 'Support', path: '/support' },
  { key: 'mobile-notifications', label: 'Mobile notifications', path: '/mobile-notifications' }
];
