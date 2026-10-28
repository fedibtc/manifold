import type { ReachCall } from '@operator-ui/types';
import { describe, expect, it } from 'vitest';
import { buildReachRows, callTone, formatDuration } from '../reachRows';

const call = (overrides: Partial<ReachCall>): ReachCall => ({
  seat_id: 's1',
  op: 'fee_status',
  last_ms: 100,
  max_ms: 100,
  last_at_ms: 0,
  count: 1,
  errors: 0,
  last_ok: true,
  ...overrides
});

describe('reachRows', () => {
  it('should format milliseconds under a second and seconds above it', () => {
    expect(formatDuration(250)).toBe('250 ms');
    expect(formatDuration(64_300)).toBe('64.3 s');
  });

  it('should treat a call at the dashboard timeout as slow and a failed one as bad', () => {
    expect(callTone(call({ last_ms: 14_999 }))).toBe('ok');
    expect(callTone(call({ last_ms: 15_000 }))).toBe('warn');
    expect(callTone(call({ last_ok: false }))).toBe('bad');
  });

  it('should pick each seat its own slowest call and ignore other seats', () => {
    const rows = buildReachRows({
      calls: [
        call({ op: 'fee_status', max_ms: 100 }),
        call({ op: 'fee_drain', max_ms: 60_000 }),
        call({ seat_id: 's2', op: 'fee_total', max_ms: 5 })
      ],
      seats: [{ seat_id: 's1', peers: [], peers_error: null }]
    });

    expect(rows).toHaveLength(1);
    expect(rows[0]?.headlineCall?.op).toBe('fee_drain');
  });

  it('should show a quick failure instead of hiding it behind a slow success', () => {
    const [row] = buildReachRows({
      calls: [
        call({ op: 'fee_drain', max_ms: 900 }),
        call({ op: 'fee_collect', max_ms: 200, last_ms: 200, last_ok: false, errors: 1 })
      ],
      seats: [{ seat_id: 's1', peers: [], peers_error: null }]
    });

    expect(row?.headlineCall?.op).toBe('fee_collect');
  });

  it('should show a call past the dashboard timeout over a faster one', () => {
    const [row] = buildReachRows({
      calls: [
        call({ op: 'fee_status', max_ms: 20_000, last_ms: 100 }),
        call({ op: 'fee_drain', max_ms: 64_000, last_ms: 64_000 })
      ],
      seats: [{ seat_id: 's1', peers: [], peers_error: null }]
    });

    expect(row?.headlineCall?.op).toBe('fee_drain');
  });

  it('should count reachable peers and report the slowest round trip', () => {
    const [row] = buildReachRows({
      calls: [],
      seats: [
        {
          seat_id: 's1',
          peers: [
            { peer_id: 1, connected: true, conn_type: 'direct', rtt_ms: 40 },
            { peer_id: 2, connected: true, conn_type: 'relay', rtt_ms: 330 },
            { peer_id: 3, connected: false, conn_type: null, rtt_ms: null }
          ],
          peers_error: null
        }
      ]
    });

    expect(row).toMatchObject({ peersTotal: 3, peersConnected: 2, slowestPeerMs: 330 });
  });
});
