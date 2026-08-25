import type { CompletionCallbackStatus, SeatStatusResponse } from '@operator-ui/types';
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { SeatDetailCard } from '../SeatDetailCard';

const runningSeat: SeatStatusResponse = {
  seat_id: 'seat-01',
  fi_id: 'fi-01',
  plan: { InfiniteBestEffort: { price_msats: 50_000_000 } },
  created_at_ms: 1_753_000_000_000,
  payment_claim: { state: 'success', at_ms: 0 },
  completion_callback: { state: 'delivered', attempts: 2, at_ms: 1_753_000_000_000 },
  decommissioned: false,
  backup: { published_at_ms: 1_753_000_100_000, archive_confirmed: true },
  report: { state: 'active', health: 'healthy', phase: 'running', invite_code: 'fed1abc' },
  guardian_fee: {
    remittance_account: '{"id":"acct1"}',
    share_matches_policy: true,
    send_ppm: 1_000,
    our_weight: 1,
    total_weight: 4
  }
};

describe('SeatDetailCard', () => {
  it('should render the FI, plan, phase and invite code for a running seat', () => {
    render(<SeatDetailCard seat={runningSeat} />);

    expect(screen.getByText('fi-01')).toBeTruthy();
    expect(screen.getByText('50,000 sats, one-time')).toBeTruthy();
    expect(screen.getByText('Running')).toBeTruthy();
    expect(screen.getByText('fed1abc')).toBeTruthy();
    expect(screen.getByText('Delivered (2 attempts)')).toBeTruthy();
  });

  it.each([
    [{ state: 'not_configured' }, 'Not configured'],
    [
      { state: 'pending', attempts: 3, next_attempt_at_ms: 1, last_reason: null },
      'Pending (3 attempts)'
    ],
    [
      { state: 'operator_blocked', attempts: 0, reason: 'http_client_unavailable' },
      'Operator blocked: http_client_unavailable'
    ],
    [{ state: 'delivered', attempts: 2, at_ms: 1 }, 'Delivered (2 attempts)'],
    [
      { state: 'terminal', attempts: 1, reason: 'hook_not_found', at_ms: 1 },
      'Terminal: hook_not_found'
    ]
  ] satisfies [
    CompletionCallbackStatus,
    string
  ][])('should project callback state %# without exposing bearer data', (completionCallback, expected) => {
    render(<SeatDetailCard seat={{ ...runningSeat, completion_callback: completionCallback }} />);

    expect(screen.getByText(expected)).toBeTruthy();
  });

  it('should render the decommissioned date and no phase for a decommissioned seat', () => {
    render(
      <SeatDetailCard
        seat={{ ...runningSeat, report: { state: 'decommissioned', at_ms: 1_753_000_000_000 } }}
      />
    );

    expect(screen.getByText('Decommissioned')).toBeTruthy();
    expect(screen.queryByText('Phase')).toBeNull();
  });
});
