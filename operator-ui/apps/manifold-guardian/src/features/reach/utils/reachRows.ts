import type { ChipTone } from '@operator-ui/common-ui';
import type { ReachCall, ShowReachResponse } from '@operator-ui/types';
import { ADMIN_REQUEST_TIMEOUT_MS } from '@/shared/api/adminCall';

export const opLabels: Record<string, string> = {
  fee_status: 'Fee status',
  fee_remittances: 'Remittance history',
  fee_total: 'Lifetime total',
  fee_drain: 'Sweep check',
  fee_collect: 'Collect',
  fee_sweep: 'Sweep'
};

export interface ReachRow {
  seatId: string;
  /** The call to show for the seat: failed first, then slow, then longest-running. */
  headlineCall: ReachCall | null;
  peersTotal: number;
  peersConnected: number;
  slowestPeerMs: number | null;
  peersError: string | null;
}

export const formatDuration = (ms: number): string =>
  ms < 1000 ? `${ms} ms` : `${(ms / 1000).toFixed(1)} s`;

/** `bad` when the last call failed or was abandoned; `warn` when it ran past the
 *  point where this dashboard stops waiting. */
export const callTone = (call: ReachCall): ChipTone => {
  if (!call.last_ok) return 'bad';
  return call.last_ms >= ADMIN_REQUEST_TIMEOUT_MS ? 'warn' : 'ok';
};

export const peersTone = (row: ReachRow): ChipTone =>
  row.peersError !== null || row.peersConnected < row.peersTotal ? 'bad' : 'ok';

const TONE_RANK: Record<ChipTone, number> = { ok: 0, info: 0, neutral: 0, warn: 1, bad: 2 };

/** The call worth showing: a failed one over a slow one over the longest-running,
 *  so a quick failure is never hidden behind a slow success. */
const headline = (calls: ReachCall[]): ReachCall | null =>
  calls.reduce<ReachCall | null>((top, call) => {
    if (top === null) return call;
    const rankGap = TONE_RANK[callTone(call)] - TONE_RANK[callTone(top)];
    return rankGap > 0 || (rankGap === 0 && call.max_ms > top.max_ms) ? call : top;
  }, null);

export const buildReachRows = (reach: ShowReachResponse): ReachRow[] =>
  reach.seats.map((seat) => {
    const rtts = seat.peers.flatMap((peer) => (peer.rtt_ms === null ? [] : [peer.rtt_ms]));
    return {
      seatId: seat.seat_id,
      headlineCall: headline(reach.calls.filter((call) => call.seat_id === seat.seat_id)),
      peersTotal: seat.peers.length,
      peersConnected: seat.peers.filter((peer) => peer.connected).length,
      slowestPeerMs: rtts.length === 0 ? null : Math.max(...rtts),
      peersError: seat.peers_error
    };
  });
