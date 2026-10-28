import type { ReachCall, ShowReachResponse } from '@operator-ui/types';
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { ReachCard } from '../ReachCard';

const SEAT = 'a'.repeat(64);

const call = (overrides: Partial<ReachCall> = {}): ReachCall => ({
  seat_id: SEAT,
  op: 'fee_drain',
  last_ms: 700,
  max_ms: 900,
  last_at_ms: Date.UTC(2026, 9, 28, 12, 0, 0),
  count: 3,
  errors: 0,
  last_ok: true,
  ...overrides
});

const reach = (overrides: Partial<ShowReachResponse> = {}): ShowReachResponse => ({
  calls: [call()],
  seats: [
    {
      seat_id: SEAT,
      peers: [
        { peer_id: 1, connected: true, conn_type: 'direct', rtt_ms: 40 },
        { peer_id: 2, connected: true, conn_type: 'relay', rtt_ms: 330 }
      ],
      peers_error: null
    }
  ],
  ...overrides
});

describe('ReachCard', () => {
  it('should show a normal call and every peer reachable', () => {
    render(<ReachCard reach={reach()} />);

    expect(screen.getByText('Normal')).toBeTruthy();
    expect(screen.getByText('All reachable')).toBeTruthy();
    expect(screen.getByText(/Slowest round trip 330 ms/)).toBeTruthy();
  });

  it('should flag a call that ran past the dashboard timeout as slow', () => {
    render(<ReachCard reach={reach({ calls: [call({ last_ms: 64_300, max_ms: 65_400 })] })} />);

    expect(screen.getByText('Slow')).toBeTruthy();
    expect(screen.getByText(/last 64\.3 s, slowest 65\.4 s/)).toBeTruthy();
  });

  it('should flag a call whose last attempt failed', () => {
    render(<ReachCard reach={reach({ calls: [call({ last_ok: false, errors: 1 })] })} />);

    expect(screen.getByText('Failed')).toBeTruthy();
    expect(screen.getByText(/1 failed of 3/)).toBeTruthy();
  });

  it('should flag a seat with an unreachable peer', () => {
    render(
      <ReachCard
        reach={reach({
          seats: [
            {
              seat_id: SEAT,
              peers: [
                { peer_id: 1, connected: true, conn_type: 'direct', rtt_ms: 40 },
                { peer_id: 2, connected: false, conn_type: null, rtt_ms: null }
              ],
              peers_error: null
            }
          ]
        })}
      />
    );

    expect(screen.getByText('Unreachable')).toBeTruthy();
    expect(screen.getByText(/1 of 2 reachable/)).toBeTruthy();
  });

  it('should say so when a seat could not be asked', () => {
    render(
      <ReachCard
        reach={reach({
          seats: [{ seat_id: SEAT, peers: [], peers_error: 'fedimintd unreachable: timed out' }]
        })}
      />
    );

    expect(screen.getByText('Not asked')).toBeTruthy();
    expect(screen.getByText('fedimintd unreachable: timed out')).toBeTruthy();
  });

  it('should not claim a normal call for a seat with none yet', () => {
    render(<ReachCard reach={reach({ calls: [] })} />);

    expect(screen.getByText('No fee calls yet')).toBeTruthy();
  });

  it('should explain an empty fleet', () => {
    render(<ReachCard reach={{ calls: [], seats: [] }} />);

    expect(screen.getByText('No running seat has a federation yet.')).toBeTruthy();
  });
});
