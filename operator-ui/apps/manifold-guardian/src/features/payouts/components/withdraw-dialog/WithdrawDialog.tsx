import { Button } from '@operator-ui/common-ui';
import { useState } from 'react';
import { useCollectGuardianFees } from '@/features/payouts/api/hooks/use-collect-guardian-fees/useCollectGuardianFees';
import { useSweepGuardianFees } from '@/features/payouts/api/hooks/use-sweep-guardian-fees/useSweepGuardianFees';
import { useSweepPaymentFees } from '@/features/payouts/api/hooks/use-sweep-payment-fees/useSweepPaymentFees';
import { DialogProgress } from '@/features/payouts/components/dialog-progress/DialogProgress';
import { DialogResult } from '@/features/payouts/components/dialog-result/DialogResult';
import { type DialogRow, DialogRows } from '@/features/payouts/components/dialog-rows/DialogRows';
import {
  MethodSwitch,
  type PayoutMethod
} from '@/features/payouts/components/method-switch/MethodSwitch';
import { PayoutDialog } from '@/features/payouts/components/payout-dialog/PayoutDialog';
import {
  describeSource,
  type PayoutSource,
  withdrawTitle
} from '@/features/payouts/utils/payoutSource';
import { describeCollection } from '@/features/payouts/utils/sweepOutcome';
import { describeActionError } from '@/shared/utils/describeActionError';
import { formatSats } from '@/shared/utils/format';
import styles from './WithdrawDialog.module.css';

/** What one balance holds, as the revenue tables read it. `null` is unknown. */
export interface WithdrawBalance {
  /** Ordinary ecash: everything one withdrawal takes. */
  availableMsat: number | null;
  /** What reaches a Lightning destination after fees. */
  sweepableMsat: number | null;
  /** Guardian fees still in the pool, which a collection moves into the
   *  balance first. Absent for seat sales. */
  poolMsat?: number | null;
}

interface WithdrawDialogProps {
  source: PayoutSource;
  balance: WithdrawBalance;
  /** The stored Lightning payout address, or null when none is saved. */
  destination: string | null;
  onClose: () => void;
  onChangeAddress: () => void;
  /** The withdrawal has a durable request; the dialog continues as its view. */
  onStarted: (requestId: string) => void;
}

// Below this a collection's mint fee, about 0.1 sat a note, is a visible slice.
const SMALL_COLLECTION_MSAT = 1_000_000;

/**
 * Step one of a withdrawal: pick Lightning or ecash and review what leaves and
 * what arrives. Pressing the primary button starts the money move and shows
 * its progress here until the daemon has recorded it, then hands over to the
 * payout's own view.
 */
export const WithdrawDialog = ({
  source,
  balance,
  destination,
  onClose,
  onChangeAddress,
  onStarted
}: WithdrawDialogProps) => {
  const [method, setMethod] = useState<PayoutMethod>(destination ? 'lightning' : 'ecash');
  const sweepPayment = useSweepPaymentFees(
    source.kind === 'payment_federation' ? source.federationId : '',
    method
  );
  const sweepGuardian = useSweepGuardianFees(
    source.kind === 'guardian_fee' ? source.seatId : '',
    method
  );
  const move = source.kind === 'payment_federation' ? sweepPayment : sweepGuardian;

  const title = withdrawTitle(source);

  const handleWithdraw = () => {
    move.mutate(undefined, { onSuccess: (job) => onStarted(job.request_id) });
  };

  if (move.isPending) {
    return (
      <PayoutDialog
        title={title}
        source={source}
        note={method === 'ecash' && 'This can take up to a minute.'}
        footer={null}
      >
        <DialogProgress
          steps={[
            {
              label: method === 'lightning' ? 'Starting the payment' : 'Creating your ecash…',
              done: false
            }
          ]}
        />
      </PayoutDialog>
    );
  }

  if (move.isError) {
    return (
      <PayoutDialog
        title={title}
        source={source}
        onDismiss={onClose}
        footer={
          <>
            <Button variant="secondary" onClick={onClose}>
              Close
            </Button>

            <Button onClick={handleWithdraw}>Try again</Button>
          </>
        }
      >
        <DialogResult tone="warning" heading="The withdrawal did not start">
          <p>{describeActionError(move.error)}</p>

          <p>Trying again continues the same withdrawal; it cannot withdraw twice.</p>
        </DialogResult>
      </PayoutDialog>
    );
  }

  const review =
    method === 'lightning'
      ? lightningReview(balance, destination, onChangeAddress)
      : ecashReview(balance);

  return (
    <PayoutDialog
      title={title}
      source={source}
      onDismiss={onClose}
      note={review.note}
      footer={
        <>
          <Button variant="secondary" onClick={onClose}>
            Cancel
          </Button>

          <Button disabled={review.blocked} onClick={handleWithdraw}>
            {review.primaryLabel}
          </Button>
        </>
      }
    >
      {source.kind === 'guardian_fee' && (
        <FeePool
          seatId={source.seatId}
          poolMsat={balance.poolMsat ?? null}
          isPrimary={balance.availableMsat === 0}
        />
      )}

      <MethodSwitch value={method} onChange={setMethod} />

      <DialogRows
        rows={[
          { label: 'From', value: describeSource(source) },
          { label: 'Balance', value: formatSats(balance.availableMsat) },
          ...review.rows
        ]}
      />
    </PayoutDialog>
  );
};

interface Review {
  rows: DialogRow[];
  note: string | null;
  primaryLabel: string;
  blocked: boolean;
}

const lightningReview = (
  balance: WithdrawBalance,
  destination: string | null,
  onChangeAddress: () => void
): Review => {
  const { availableMsat, sweepableMsat } = balance;
  const tooSmall = availableMsat !== null && availableMsat > 0 && sweepableMsat === 0;
  const estimated = availableMsat !== null && sweepableMsat !== null;
  const to = destination ? (
    <span className={styles.toRow}>
      {destination}
      <button type="button" className={styles.link} onClick={onChangeAddress}>
        Change
      </button>
    </span>
  ) : (
    <span className={styles.toRow}>
      No payout address yet.
      <Button size="small" variant="secondary" onClick={onChangeAddress}>
        Add payout address
      </Button>
    </span>
  );
  return {
    rows: [
      { label: 'To', value: to },
      estimated
        ? {
            label: 'Fees',
            value: `≈ ${formatSats(availableMsat - sweepableMsat)}`,
            caption: 'Lightning and federation fees'
          }
        : { label: 'Fees', value: "Can't estimate right now" },
      ...(estimated && sweepableMsat > 0
        ? [
            {
              label: 'Total',
              value: <strong>You receive about {formatSats(sweepableMsat)}</strong>
            }
          ]
        : [])
    ],
    note: tooSmall
      ? 'This balance is too small to send over Lightning after fees. Withdraw it as ecash instead.'
      : null,
    primaryLabel:
      sweepableMsat !== null && sweepableMsat > 0
        ? `Withdraw ${formatSats(sweepableMsat)}`
        : 'Withdraw over Lightning',
    blocked: destination === null || availableMsat === 0 || tooSmall
  };
};

const ecashReview = (balance: WithdrawBalance): Review => ({
  rows: [
    {
      label: 'You get',
      value:
        balance.availableMsat === null
          ? 'One ecash code for the whole balance'
          : `One ecash code for ${formatSats(balance.availableMsat)}`
    },
    {
      label: 'Fees',
      value: 'None to create',
      caption: 'Your wallet may pay a small federation fee when it redeems.'
    },
    { label: 'Total', value: <strong>Redeem it in your Fedi wallet, in this federation</strong> }
  ],
  note: 'The whole balance leaves at once. Anyone who sees the code can take the money, so open it only when you are ready to scan.',
  primaryLabel:
    balance.availableMsat === null
      ? 'Withdraw as ecash'
      : `Withdraw ${formatSats(balance.availableMsat)} as ecash`,
  blocked: balance.availableMsat === 0
});

interface FeePoolProps {
  seatId: string;
  poolMsat: number | null;
  /** Nothing is ready to withdraw yet, so collecting is the next step. */
  isPrimary: boolean;
}

/**
 * Guardian fees still in the pool are not in the balance a withdrawal takes. A
 * collection moves what the pool releases into it, in place: the review's
 * numbers update when the balance is read again. Locked deposits stay until the
 * next cycle turnover, which the outcome states.
 */
const FeePool = ({ seatId, poolMsat, isPrimary }: FeePoolProps) => {
  const collect = useCollectGuardianFees(seatId);
  if (!poolMsat && !collect.isSuccess) return null;

  return (
    <div className={styles.pool}>
      {collect.isSuccess ? (
        <p className={styles.poolText} role="status">
          {describeCollection(collect.data)}
        </p>
      ) : (
        <>
          <p className={styles.poolText}>
            {formatSats(poolMsat)} are still in the fee pool. Collect them to include them in this
            withdrawal.
          </p>

          <div>
            <Button
              size="small"
              variant={isPrimary ? 'primary' : 'secondary'}
              loading={collect.isPending}
              onClick={() => collect.mutate()}
            >
              Collect {formatSats(poolMsat)}
            </Button>
          </div>
          {poolMsat !== null && poolMsat < SMALL_COLLECTION_MSAT && (
            <span className={styles.caption}>
              Minting the ecash costs about 0.1 sat for every note it creates, so a small collection
              loses proportionally more of itself.
            </span>
          )}
        </>
      )}

      {collect.isError && (
        <span role="alert" className={styles.error}>
          {describeActionError(collect.error)}
        </span>
      )}
    </div>
  );
};
