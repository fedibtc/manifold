import {
  type Column,
  CopyButton,
  DataTable,
  isTruncated,
  SectionCard,
  truncateMiddle
} from '@operator-ui/common-ui';
import type { PaymentFederation } from '@operator-ui/types';
import { PaymentSweepAction } from '@/features/payouts/components/payment-sweep-action/PaymentSweepAction';
import { formatSats } from '@/shared/utils/format';
import styles from './PaymentSweepTable.module.css';

interface PaymentSweepTableProps {
  federations: PaymentFederation[];
  hasDestination: boolean;
}

const federationRowKey = (federation: PaymentFederation) => federation.federation_id;

/**
 * Setup-payment revenue, which is per payment federation and leaves in one step.
 * Former members are listed too: membership is the authenticated common set
 * rather than an operator choice, and a leftover balance is still the operator's
 * money to move.
 */
export const PaymentSweepTable = ({ federations, hasDestination }: PaymentSweepTableProps) => {
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
      key: 'balance',
      header: 'Balance',
      render: (federation) => formatSats(federation.wallet.available_ecash_msat)
    },
    {
      key: 'sweep',
      header: 'Send to destination',
      render: (federation) => (
        <PaymentSweepAction
          federationId={federation.federation_id}
          balanceMsat={federation.wallet.available_ecash_msat}
          hasDestination={hasDestination}
        />
      )
    }
  ];

  return (
    <SectionCard title="Setup-payment revenue" frame="table">
      <DataTable columns={columns} rows={federations} rowKey={federationRowKey} />
    </SectionCard>
  );
};
