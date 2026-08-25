import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import type { SourceRow } from '@/features/funds/utils/deriveFunds';
import { SourcesTable } from '../SourcesTable';

const rows: SourceRow[] = [
  { key: 'gateway', source: 'Mock Signet Gateway', available: 3_000_000, status: 'available' },
  { key: 'stability_pool', source: 'Stability pool', available: 250_000, status: 'unavailable' }
];

describe('SourcesTable', () => {
  it('should render a row per liquidity source with its available amount', () => {
    render(<SourcesTable rows={rows} />);

    expect(screen.getByText('Mock Signet Gateway')).toBeTruthy();
    expect(screen.getByText('3,000,000 sats')).toBeTruthy();
    expect(screen.getByText('Stability pool')).toBeTruthy();
    expect(screen.getByText('250,000 sats')).toBeTruthy();
  });

  it('should render the humanized status for each source', () => {
    render(<SourcesTable rows={rows} />);

    expect(screen.getByText('available')).toBeTruthy();
    expect(screen.getByText('unavailable')).toBeTruthy();
  });
});
