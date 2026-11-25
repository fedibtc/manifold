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
import { WithdrawCell } from '@/features/payouts/components/withdraw-cell/WithdrawCell';
import { formatSats } from '@/shared/utils/format';
import styles from './PaymentSweepTable.module.css';

interface PaymentSweepTableProps {
  federations: PaymentFederation[];
  onWithdraw: (federationId: string) => void;
}

const federationRowKey = (federation: PaymentFederation) => federation.federation_id;

/**
 * Setup-payment revenue, which is per payment federation and leaves in one step.
 * Former members are listed too: membership is the authenticated common set
 * rather than an operator choice, and a leftover balance is still the operator's
 * money to move.
 *
 * The status column used to live on a separate Wallet screen that restated this
 * federation list without offering an action on it. It belongs beside the
 * withdraw button instead: "former member" and "not accepting payments" are the
 * two answers to the question a stranded balance raises.
 */
export const PaymentSweepTable = ({ federations, onWithdraw }: PaymentSweepTableProps) => {
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
      key: 'status',
      header: 'Status',
      render: (federation) => {
        if (!federation.accepted) return <Chip tone="neutral">Former member</Chip>;
        return federation.receivable ? (
          <Chip tone="ok">Accepting payments</Chip>
        ) : (
          <Chip tone="warn">Not accepting payments</Chip>
        );
      }
    },
    {
      key: 'balance',
      header: 'Available',
      render: (federation) => formatSats(federation.wallet.available_ecash_msat)
    },
    {
      key: 'withdraw',
      header: '',
      render: (federation) => (
        <WithdrawCell
          isEmpty={federation.wallet.available_ecash_msat === 0}
          balanceName={`seat sales from ${truncateMiddle(federation.federation_id, 8, 8)}`}
          onWithdraw={() => onWithdraw(federation.federation_id)}
        />
      )
    }
  ];

  return (
    <SectionCard title="Seat sales" frame="table">
      {federations.length === 0 ? (
        <p className={styles.empty}>No payment federations accepted yet.</p>
      ) : (
        <DataTable columns={columns} rows={federations} rowKey={federationRowKey} />
      )}
    </SectionCard>
  );
};
