import { Button, Chip, SectionCard } from '@operator-ui/common-ui';
import type { PayoutJobStatusResponse } from '@operator-ui/types';
import { describeSource, sourceOfScope } from '@/features/payouts/utils/payoutSource';
import { payoutStage } from '@/features/payouts/utils/payoutStage';
import { formatSats } from '@/shared/utils/format';
import styles from './InProgressCard.module.css';

interface InProgressCardProps {
  /** Payouts that are not final yet, newest first. */
  payouts: PayoutJobStatusResponse[];
  /** Open one payout's dialog: for an issued token, its code. */
  onOpen: (requestId: string) => void;
}

/**
 * Withdrawals that are not final, one compact row each. No token and no QR
 * code sit on the page: an issued token shows only in its dialog, behind
 * `Show ecash`.
 */
export const InProgressCard = ({ payouts, onOpen }: InProgressCardProps) => (
  <SectionCard title="In progress" frame="table">
    <ul className={styles.list}>
      {payouts.map((payout) => {
        const stage = payoutStage(payout);
        const amount = formatSats(payout.job.operation?.amount_msat);
        const isIssued = 'ecash' in payout && payout.ecash?.state === 'issued';
        return (
          <li key={payout.job.request_id} className={styles.row}>
            <span className={styles.amount}>{amount}</span>

            <span className={styles.how}>
              {payout.job.method === 'lightning'
                ? `Lightning to ${payout.job.destination}`
                : 'Ecash'}
            </span>

            <span className={styles.source}>{describeSource(sourceOfScope(payout.job.scope))}</span>

            <Chip tone={stage.tone}>{stage.label}</Chip>

            <span className={styles.action}>
              {isIssued && (
                <Button
                  size="small"
                  variant="secondary"
                  onClick={() => onOpen(payout.job.request_id)}
                >
                  Show ecash
                </Button>
              )}
            </span>
          </li>
        );
      })}
    </ul>
  </SectionCard>
);
