import {
  Button,
  Chip,
  type ChipTone,
  type Column,
  CopyButton,
  DataTable,
  isTruncated,
  SectionCard,
  truncateMiddle
} from '@operator-ui/common-ui';
import type { WalletOperationStatus, WalletOperationSummary } from '@operator-ui/types';
import { formatSats, humanizeToken } from '@/shared/utils/format';
import styles from './WalletOperationsTable.module.css';

const OPERATION_TONES: Record<WalletOperationStatus, ChipTone> = {
  pending: 'warn',
  broadcast: 'info',
  confirmed: 'info',
  completed: 'ok',
  failed: 'bad',
  in_doubt: 'warn',
  manual_review_required: 'warn',
  cancelled: 'neutral'
};

// Only the frozen ones. `manual_review_required` is the one status nothing but
// an operator moves: the sync pass skips it and retry refuses it, so a row in
// that state is a stuck payment and this is the way out of it. Every other
// status either advances on its own or is already terminal.
const operationColumns = (
  onResolve: ((operationId: string) => void) | undefined
): Column<WalletOperationSummary>[] => [
  {
    key: 'operation',
    header: 'Operation',
    render: (row) => (
      <span className={styles.idRow}>
        {truncateMiddle(row.operation_id, 8, 8)}
        {isTruncated(row.operation_id, 8, 8) && (
          <CopyButton value={row.operation_id} label="Copy operation ID" />
        )}
      </span>
    )
  },
  { key: 'type', header: 'Type', render: (row) => humanizeToken(row.operation_type) },
  { key: 'amount', header: 'Amount', render: (row) => formatSats(row.amount) },
  {
    key: 'status',
    header: 'Status',
    render: (row) => <Chip tone={OPERATION_TONES[row.status]}>{humanizeToken(row.status)}</Chip>
  },
  {
    key: 'action',
    header: '',
    render: (row) =>
      onResolve && row.status === 'manual_review_required' ? (
        <Button variant="secondary" size="small" onClick={() => onResolve(row.operation_id)}>
          Resolve
        </Button>
      ) : null
  }
];

const operationRowKey = (row: WalletOperationSummary) => row.operation_id;

interface WalletOperationsTableProps {
  operations: WalletOperationSummary[];
  onResolve?: (operationId: string) => void;
}

export const WalletOperationsTable = ({ operations, onResolve }: WalletOperationsTableProps) => (
  <SectionCard title="Wallet operations" frame="table">
    {operations.length > 0 ? (
      <DataTable columns={operationColumns(onResolve)} rows={operations} rowKey={operationRowKey} />
    ) : (
      <p className={styles.empty}>No wallet operations yet.</p>
    )}
  </SectionCard>
);
