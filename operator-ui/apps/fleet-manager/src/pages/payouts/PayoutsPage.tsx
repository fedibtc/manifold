import { usePayoutDestination } from '@/features/payouts/api/hooks/use-payout-destination/usePayoutDestination';
import { GuardianFeeTable } from '@/features/payouts/components/guardian-fee-table/GuardianFeeTable';
import { PaymentSweepTable } from '@/features/payouts/components/payment-sweep-table/PaymentSweepTable';
import { PayoutDestinationCard } from '@/features/payouts/components/payout-destination-card/PayoutDestinationCard';
import { useGuardianFeeRows } from '@/features/payouts/hooks/use-guardian-fee-rows/useGuardianFeeRows';
import { usePaymentFederations } from '@/shared/api/hooks/use-payment-federations/usePaymentFederations';
import { useSeats } from '@/shared/api/hooks/use-seats/useSeats';
import { QuerySurface } from '@/shared/components/query-surface/QuerySurface';
import { useQueryDisposition } from '@/shared/query/use-query-disposition/useQueryDisposition';
import styles from './PayoutsPage.module.css';

/**
 * The fleet's only money-out surface.
 *
 * It is ordered the way the daemon is: the destination first, because every
 * sweep refuses without one, then the two revenue sources — which are not
 * symmetric and are deliberately not presented as one list. Setup-payment
 * revenue is per federation and leaves in one step; guardian-fee revenue is per
 * seat and leaves in two, because what the pool releases has to become ordinary
 * ecash before it can be sent.
 */
export const PayoutsPage = () => {
  const destination = usePayoutDestination();
  const paymentFederations = usePaymentFederations();
  const seats = useSeats();
  const guardianFeeRows = useGuardianFeeRows();

  // The per-seat fee reads are deliberately outside this: one seat's fee account
  // failing is a fact about that row, stated as "—" in it, not an outage of the
  // whole screen.
  const { disposition, retry } = useQueryDisposition([destination, paymentFederations, seats]);
  const storedDestination = destination.data?.destination ?? null;

  return (
    <div className={styles.root}>
      <div className={styles.head}>
        <h1 className={styles.heading}>Payouts</h1>

        <p className={styles.intro}>
          A sweep sends the largest amount the wallet can economically fund, through a gateway the
          daemon selects. There is no amount to enter and no gateway to pick.
        </p>
      </div>

      <QuerySurface disposition={disposition} onRetry={retry}>
        <PayoutDestinationCard destination={storedDestination} />

        <PaymentSweepTable
          federations={paymentFederations.data?.federations ?? []}
          hasDestination={storedDestination !== null}
        />

        <GuardianFeeTable rows={guardianFeeRows} hasDestination={storedDestination !== null} />
      </QuerySurface>
    </div>
  );
};
