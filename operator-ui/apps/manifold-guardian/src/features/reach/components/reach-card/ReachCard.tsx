import {
  Chip,
  type Column,
  CopyButton,
  DataTable,
  isTruncated,
  truncateMiddle
} from '@operator-ui/common-ui';
import type { ShowReachResponse } from '@operator-ui/types';
import {
  buildReachRows,
  callTone,
  formatDuration,
  opLabels,
  peersTone,
  type ReachRow
} from '@/features/reach/utils/reachRows';
import { formatCheckedAt } from '@/shared/utils/format';
import styles from './ReachCard.module.css';

interface ReachCardProps {
  reach: ShowReachResponse;
}

const reachRowKey = (row: ReachRow) => row.seatId;

const callChip = { ok: 'Normal', warn: 'Slow', bad: 'Failed' } as const;

const columns: Column<ReachRow>[] = [
  {
    key: 'seat',
    header: 'Seat',
    render: (row) => (
      <span className={styles.idRow}>
        <span className={styles.mono}>{truncateMiddle(row.seatId, 8, 8)}</span>

        {isTruncated(row.seatId, 8, 8) && <CopyButton value={row.seatId} label="Copy seat ID" />}
      </span>
    )
  },
  {
    key: 'calls',
    header: 'Fee calls',
    render: (row) => {
      const call = row.headlineCall;
      if (call === null) return <span className={styles.muted}>No fee calls yet</span>;
      const tone = callTone(call);
      return (
        <span className={styles.cell}>
          <span>
            {opLabels[call.op] ?? call.op}: last {formatDuration(call.last_ms)}, slowest{' '}
            {formatDuration(call.max_ms)}{' '}
            <Chip tone={tone}>{callChip[tone as keyof typeof callChip]}</Chip>
          </span>

          <span className={styles.muted}>
            {call.errors} failed of {call.count}, last at{' '}
            {formatCheckedAt(Math.floor(call.last_at_ms / 1000))}
          </span>
        </span>
      );
    }
  },
  {
    key: 'peers',
    header: 'Fellow guardians',
    render: (row) => {
      if (row.peersError !== null) {
        return (
          <span className={styles.cell}>
            <Chip tone="bad">Not asked</Chip>

            <span className={styles.muted}>{row.peersError}</span>
          </span>
        );
      }
      return (
        <span className={styles.cell}>
          <span>
            {row.peersConnected} of {row.peersTotal} reachable{' '}
            <Chip tone={peersTone(row)}>
              {peersTone(row) === 'ok' ? 'All reachable' : 'Unreachable'}
            </Chip>
          </span>
          {row.slowestPeerMs !== null && (
            <span className={styles.muted}>
              Slowest round trip {formatDuration(row.slowestPeerMs)}
            </span>
          )}
        </span>
      );
    }
  }
];

/**
 * Per running seat: how long the guardian-fee calls to its federation take, and
 * whether its own fedimintd reaches the other guardians. Call timings fill in
 * as the fee screens are used; they restart empty with the daemon.
 */
export const ReachCard = ({ reach }: ReachCardProps) => {
  const rows = buildReachRows(reach);

  return rows.length === 0 ? (
    <p className={styles.empty}>No running seat has a federation yet.</p>
  ) : (
    <DataTable columns={columns} rows={rows} rowKey={reachRowKey} />
  );
};
