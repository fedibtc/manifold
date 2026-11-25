import type { PaymentFederation } from '@operator-ui/types';
import { useState } from 'react';
import { usePayoutDestination } from '@/features/payouts/api/hooks/use-payout-destination/usePayoutDestination';
import { usePayouts } from '@/features/payouts/api/hooks/use-payouts/usePayouts';
import { GuardianFeeTable } from '@/features/payouts/components/guardian-fee-table/GuardianFeeTable';
import { InProgressCard } from '@/features/payouts/components/in-progress-card/InProgressCard';
import { PaymentSweepTable } from '@/features/payouts/components/payment-sweep-table/PaymentSweepTable';
import { PayoutAddressDialog } from '@/features/payouts/components/payout-address-dialog/PayoutAddressDialog';
import { PayoutAddressLine } from '@/features/payouts/components/payout-address-line/PayoutAddressLine';
import { PayoutHistoryTable } from '@/features/payouts/components/payout-history-table/PayoutHistoryTable';
import { PayoutView } from '@/features/payouts/components/payout-view/PayoutView';
import {
  type WithdrawBalance,
  WithdrawDialog
} from '@/features/payouts/components/withdraw-dialog/WithdrawDialog';
import {
  type GuardianFeeRow,
  useGuardianFeeRows
} from '@/features/payouts/hooks/use-guardian-fee-rows/useGuardianFeeRows';
import type { PayoutSource } from '@/features/payouts/utils/payoutSource';
import { payoutStage } from '@/features/payouts/utils/payoutStage';
import { usePaymentFederations } from '@/shared/api/hooks/use-payment-federations/usePaymentFederations';
import { useSeats } from '@/shared/api/hooks/use-seats/useSeats';
import { QuerySurface } from '@/shared/components/query-surface/QuerySurface';
import { useQueryDisposition } from '@/shared/query/use-query-disposition/useQueryDisposition';
import styles from './PayoutsPage.module.css';

/** The one dialog the page shows, if any. */
type PayoutsDialog =
  | { kind: 'withdraw'; source: PayoutSource }
  | { kind: 'payout'; requestId: string }
  | { kind: 'address'; returnTo: PayoutsDialog | null };

const UNKNOWN_BALANCE: WithdrawBalance = { availableMsat: null, sweepableMsat: null };

const balanceOf = (
  source: PayoutSource,
  federations: PaymentFederation[],
  feeRows: GuardianFeeRow[]
): WithdrawBalance => {
  if (source.kind === 'payment_federation') {
    const wallet = federations.find(
      (federation) => federation.federation_id === source.federationId
    )?.wallet;
    return wallet
      ? {
          availableMsat: wallet.available_ecash_msat,
          sweepableMsat: wallet.economically_sweepable_recipient_msat
        }
      : UNKNOWN_BALANCE;
  }
  const row = feeRows.find((candidate) => candidate.seatId === source.seatId);
  return row
    ? {
        availableMsat: row.collectedEcashMsat,
        sweepableMsat: row.sweepableMsat,
        poolMsat: row.collectableMsat
      }
    : UNKNOWN_BALANCE;
};

/**
 * The fleet's only money-out surface. Each balance row has one Withdraw button
 * that opens one dialog for the whole move, Lightning or ecash: review,
 * progress, result. Withdrawals that are not final sit at the top, finished
 * ones in History, both read from the daemon's payout list.
 */
export const PayoutsPage = () => {
  const destination = usePayoutDestination();
  const paymentFederations = usePaymentFederations();
  const seats = useSeats();
  const guardianFeeRows = useGuardianFeeRows();
  const payouts = usePayouts();
  const [dialog, setDialog] = useState<PayoutsDialog | null>(null);

  // The per-seat fee reads are deliberately outside this: one seat's fee account
  // failing is a fact about that row, stated as "—" in it, not an outage of the
  // whole screen.
  const { disposition, retry } = useQueryDisposition([destination, paymentFederations, seats]);
  const payoutsRead = useQueryDisposition([payouts]);
  const storedDestination = destination.data?.destination ?? null;
  const federations = paymentFederations.data?.federations ?? [];
  const listed = payouts.data?.payouts ?? [];
  const inProgress = listed.filter((payout) => payoutStage(payout).open);
  const history = listed.filter((payout) => !payoutStage(payout).open);

  const close = () => setDialog(null);
  const withdraw = (source: PayoutSource) => setDialog({ kind: 'withdraw', source });
  const open = (requestId: string) => setDialog({ kind: 'payout', requestId });

  return (
    <div className={styles.root}>
      <div className={styles.head}>
        <h1 className={styles.heading}>Payouts</h1>

        <p className={styles.intro}>
          Withdraw your earnings over Lightning to your payout address, or as ecash you scan with
          your Fedi wallet.
        </p>
      </div>

      {inProgress.length > 0 && <InProgressCard payouts={inProgress} onOpen={open} />}

      <QuerySurface disposition={disposition} onRetry={retry}>
        <PayoutAddressLine
          destination={storedDestination}
          onChange={() => setDialog({ kind: 'address', returnTo: null })}
        />

        <PaymentSweepTable
          federations={federations}
          onWithdraw={(federationId) => withdraw({ kind: 'payment_federation', federationId })}
        />

        <GuardianFeeTable
          rows={guardianFeeRows}
          onWithdraw={(seatId) => withdraw({ kind: 'guardian_fee', seatId })}
        />
      </QuerySurface>

      <QuerySurface disposition={payoutsRead.disposition} onRetry={payoutsRead.retry}>
        <PayoutHistoryTable payouts={history} onOpen={open} />
      </QuerySurface>

      {dialog?.kind === 'withdraw' && (
        <WithdrawDialog
          source={dialog.source}
          balance={balanceOf(dialog.source, federations, guardianFeeRows)}
          destination={storedDestination}
          onClose={close}
          onChangeAddress={() => setDialog({ kind: 'address', returnTo: dialog })}
          onStarted={open}
        />
      )}

      {dialog?.kind === 'payout' && (
        <PayoutView requestId={dialog.requestId} onClose={close} onWithdrawAgain={withdraw} />
      )}

      {dialog?.kind === 'address' && (
        <PayoutAddressDialog
          destination={storedDestination}
          onClose={() => setDialog(dialog.returnTo)}
          onDone={() => setDialog(dialog.returnTo)}
        />
      )}
    </div>
  );
};
