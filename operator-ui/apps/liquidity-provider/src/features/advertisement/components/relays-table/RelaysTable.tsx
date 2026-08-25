import {
  type Column,
  CopyButton,
  DataTable,
  isTruncated,
  SectionCard,
  truncateMiddle
} from '@operator-ui/common-ui';
import type { RelayPublicationState, RelayStatus } from '@operator-ui/types';
import { formatRelative } from '../../services/format';
import { type ChipTone, StatusChip } from '../status-chip/StatusChip';
import styles from './RelaysTable.module.css';

interface RelaysTableProps {
  relays: RelayPublicationState[];
  now: number;
}

const RELAY_TONE: Record<RelayStatus, ChipTone> = {
  connected: 'ok',
  published: 'ok',
  disconnected: 'bad',
  failed: 'bad'
};

const RELAY_LABEL: Record<RelayStatus, string> = {
  connected: 'Connected',
  published: 'Published',
  disconnected: 'Disconnected',
  failed: 'Failed'
};

const relayStatusLabel = (relay: RelayPublicationState): string =>
  relay.last_error
    ? `${RELAY_LABEL[relay.status]} · ${relay.last_error}`
    : RELAY_LABEL[relay.status];

export const RelaysTable = ({ relays, now }: RelaysTableProps) => {
  const columns: Column<RelayPublicationState>[] = [
    {
      key: 'relay',
      header: 'Relay',
      render: (row) => (
        <span className={styles.idRow}>
          <span className={styles.mono}>{truncateMiddle(row.relay_url, 14, 10)}</span>

          {isTruncated(row.relay_url, 14, 10) && (
            <CopyButton value={row.relay_url} label="Copy relay URL" />
          )}
        </span>
      )
    },
    {
      key: 'status',
      header: 'Status',
      render: (row) => (
        <StatusChip tone={RELAY_TONE[row.status]}>{relayStatusLabel(row)}</StatusChip>
      )
    },
    {
      key: 'lastSeen',
      header: 'Last seen',
      render: (row) => formatRelative(row.last_seen_at, now)
    }
  ];

  return (
    <div className={styles.root}>
      <SectionCard title="Relays" frame="table">
        <DataTable columns={columns} rows={relays} rowKey={(row) => row.relay_url} />
      </SectionCard>

      <p className={styles.note}>
        One relay down is fine — the listing stays discoverable while at least one relay carries it.
      </p>
    </div>
  );
};
