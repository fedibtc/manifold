import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import type { ActivityRow } from '@/features/overview/utils/derive';
import { ActivityTable } from '../ActivityTable';

const rows: ActivityRow[] = [
  {
    key: 'wop-0003',
    when: '5m ago',
    event: 'deposit',
    amount: '1,000,000 sats',
    status: 'pending'
  },
  {
    key: 'alloc-1',
    when: '2h ago',
    event: 'allocation',
    amount: '250,000 sats',
    status: 'completed'
  }
];

describe('ActivityTable', () => {
  it('should render the section title', () => {
    render(<ActivityTable rows={rows} />);

    expect(screen.getByRole('heading', { name: 'Recent activity' })).toBeTruthy();
  });

  it('should render a row per activity entry', () => {
    render(<ActivityTable rows={rows} />);

    expect(screen.getByText('deposit')).toBeTruthy();
    expect(screen.getByText('1,000,000 sats')).toBeTruthy();
    expect(screen.getByText('allocation')).toBeTruthy();
    expect(screen.getByText('5m ago')).toBeTruthy();
  });

  it('should render the empty note when there are no rows', () => {
    render(<ActivityTable rows={[]} />);

    expect(screen.getByText('No recent activity yet.')).toBeTruthy();
    expect(screen.queryByRole('table')).toBeNull();
  });
});
