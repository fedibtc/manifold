import {
  Chip,
  type Column,
  CopyButton,
  DataTable,
  isTruncated,
  SectionCard,
  truncateMiddle
} from '@operator-ui/common-ui';
import type { PaymentFederation } from '@operator-ui/types';
import { formatSats } from '@/shared/utils/format';
import styles from './FederationTable.module.css';

const columns: Column<PaymentFederation>[] = [
  {
    key: 'federation',
    header: 'Federation',
    render: (federation) => (
      <span className={styles.idRow}>
        <span className={styles.mono}>{truncateMiddle(federation.federation_id, 8, 8)}</span>

        {isTruncated(federation.federation_id, 8, 8) && (
          <CopyButton value={federation.federation_id} label="Copy federation ID" />
        )}
      </span>
    )
  },
  {
    key: 'receivable',
    header: 'Status',
    render: (federation) => {
      // Membership is the authenticated common set, not an operator choice — a
      // federation that is no longer accepted still shows, because its balance
      // is still the operator's money.
      if (!federation.accepted) return <Chip tone="neutral">Former member</Chip>;
      return federation.receivable ? (
        <Chip tone="ok">Receivable</Chip>
      ) : (
        <Chip tone="warn">Not receiving</Chip>
      );
    }
  },
  {
    key: 'balance',
    header: 'Balance',
    render: (federation) => formatSats(federation.wallet.available_ecash_msat)
  }
];

const federationRowKey = (federation: PaymentFederation) => federation.federation_id;

interface FederationTableProps {
  federations: PaymentFederation[];
}

export const FederationTable = ({ federations }: FederationTableProps) => (
  <SectionCard title="Federations" frame="table">
    <DataTable columns={columns} rows={federations} rowKey={federationRowKey} />
  </SectionCard>
);
