import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { describe, expect, it } from 'vitest';
import type { SeatRow } from '@/features/seats/hooks/use-seat-rows/useSeatRows';
import { SeatTable } from '../SeatTable';

const rows: SeatRow[] = [
  {
    seat: {
      seat_id: 'seat-01',
      fi_id: 'fi-01',
      plan: { InfiniteBestEffort: { price_msats: 50_000_000 } },
      created_at_ms: 1_753_000_000_000,
      payment_claim: { state: 'success', at_ms: 0 },
      completion_callback: { state: 'not_configured' },
      decommissioned: false,
      backup: null
    },
    report: {
      state: 'active',
      health: 'healthy',
      phase: 'running',
      invite_code: 'fed11testinvite'
    },
    reportLoading: false
  },
  {
    seat: {
      seat_id: 'seat-02',
      fi_id: 'fi-02',
      plan: { InfiniteBestEffort: { price_msats: 100_000 } },
      created_at_ms: 1_753_000_000_000,
      payment_claim: { state: 'success', at_ms: 0 },
      completion_callback: { state: 'not_configured' },
      decommissioned: true,
      backup: null
    },
    reportLoading: false
  }
];

const renderTable = () =>
  render(
    <MemoryRouter>
      <SeatTable rows={rows} />
    </MemoryRouter>
  );

describe('SeatTable', () => {
  it('should link each seat to its detail page', () => {
    renderTable();

    const link = screen.getByRole('link', { name: 'seat-01' });
    expect(link.getAttribute('href')).toBe('/seats/seat-01');
  });

  it('should render the plan, phase and health for an active seat', () => {
    renderTable();

    expect(screen.getByText('fi-01')).toBeTruthy();
    expect(screen.getByText('50,000 sats, one-time')).toBeTruthy();
    expect(screen.getByText('Running')).toBeTruthy();
    expect(screen.getByText('Healthy')).toBeTruthy();
  });

  it('should dash out phase and health for a decommissioned seat', () => {
    renderTable();

    expect(screen.getAllByText('—').length).toBeGreaterThanOrEqual(2);
  });
});
