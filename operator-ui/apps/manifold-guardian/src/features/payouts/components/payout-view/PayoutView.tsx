import { Button, Chip, CopyButton, truncateMiddle } from '@operator-ui/common-ui';
import type { PayoutJobStatusResponse } from '@operator-ui/types';
import { type ReactNode, useState } from 'react';
import { useCancelEcashPayout } from '@/features/payouts/api/hooks/use-cancel-ecash-payout/useCancelEcashPayout';
import { useConfirmEcashPayout } from '@/features/payouts/api/hooks/use-confirm-ecash-payout/useConfirmEcashPayout';
import { usePayoutStatus } from '@/features/payouts/api/hooks/use-payout-status/usePayoutStatus';
import { DialogProgress } from '@/features/payouts/components/dialog-progress/DialogProgress';
import { DialogResult } from '@/features/payouts/components/dialog-result/DialogResult';
import { EcashQrCode } from '@/features/payouts/components/ecash-qr-code/EcashQrCode';
import { PayoutDialog } from '@/features/payouts/components/payout-dialog/PayoutDialog';
import {
  type PayoutSource,
  sourceOfScope,
  withdrawTitle
} from '@/features/payouts/utils/payoutSource';
import { describeActionError } from '@/shared/utils/describeActionError';
import { formatLocalMoment, formatSats } from '@/shared/utils/format';
import styles from './PayoutView.module.css';

interface PayoutViewProps {
  requestId: string;
  onClose: () => void;
  /** Start a new withdrawal from the same balance: the rest after a capped
   *  payment, or another try after a failed one. */
  onWithdrawAgain: (source: PayoutSource) => void;
}

/** The step one payout has reached, with what that step shows. */
type View =
  | { kind: 'paying'; destination: string; unknown: boolean }
  | {
      kind: 'sent';
      destination: string;
      capped: { maximum_msat: number; remaining_msat: number } | null;
    }
  | { kind: 'failed' }
  | { kind: 'not_started' }
  | { kind: 'code'; token: string }
  | { kind: 'confirm_cancel' }
  | { kind: 'returning'; committing: boolean }
  | { kind: 'redeemed'; confirmedAtMs: number }
  | { kind: 'returned' }
  | { kind: 'already_redeemed' };

/** What every step can read and do. */
interface StepContext {
  amount: string;
  operationId: string | null;
  failure: ReactNode;
  close: () => void;
  withdrawAgain: () => void;
  confirm: { run: () => void; pending: boolean };
  askCancel: () => void;
  keep: () => void;
  cancel: () => void;
}

/** One step's dialog content. A step closes with Done and can be dismissed,
 *  unless it says otherwise. */
interface StepContent {
  body: ReactNode;
  footer?: ReactNode;
  footerStart?: ReactNode;
  note?: ReactNode;
  wide?: boolean;
  /** Escape goes back to the code instead of closing. */
  onDismiss?: () => void;
  /** A cancel is committing: the step cannot be closed. */
  locked?: boolean;
}

/** Where the payout stands, read off the daemon's answer alone. */
const viewOf = (
  payout: PayoutJobStatusResponse,
  askingCancel: boolean,
  cancelling: boolean
): View => {
  if ('payout' in payout) {
    const { destination, operation } = payout.job;
    const state = payout.payout?.state;
    switch (state) {
      case undefined:
        return { kind: 'not_started' };
      case 'succeeded':
        return { kind: 'sent', destination, capped: operation?.capped ?? null };
      case 'failed_or_refunded':
        return { kind: 'failed' };
      default:
        return { kind: 'paying', destination, unknown: state === 'unknown' };
    }
  }
  if (cancelling) return { kind: 'returning', committing: true };
  if (askingCancel) return { kind: 'confirm_cancel' };
  const confirmedAtMs = payout.job.operation?.confirmed_at_ms;
  switch (payout.ecash?.state) {
    case undefined:
      return { kind: 'not_started' };
    case 'issued':
      if (confirmedAtMs) return { kind: 'redeemed', confirmedAtMs };
      // PayoutStatus always carries an issued payout's token.
      return payout.ecash.token === null
        ? { kind: 'not_started' }
        : { kind: 'code', token: payout.ecash.token };
    case 'reclaiming':
      return { kind: 'returning', committing: false };
    case 'reclaimed':
      return { kind: 'returned' };
    case 'already_spent':
      return { kind: 'already_redeemed' };
  }
};

const OperationId = ({ id }: { id: string | null }) =>
  id && (
    <span className={styles.idRow}>
      <span>Operation</span>

      <span className={styles.mono}>{truncateMiddle(id, 8, 8)}</span>

      <CopyButton value={id} label="Copy operation ID" />
    </span>
  );

/** A final outcome: what happened, what it means, and the operation. */
const receipt = (
  ctx: StepContext,
  heading: string,
  text: ReactNode,
  tone: 'success' | 'warning' = 'success'
) => (
  <DialogResult tone={tone} heading={heading}>
    <p>{text}</p>

    <OperationId id={ctx.operationId} />
  </DialogResult>
);

const closeButton = (ctx: StepContext) => (
  <Button variant="secondary" onClick={ctx.close}>
    Close
  </Button>
);

const retryFooter = (ctx: StepContext) => (
  <>
    {closeButton(ctx)}

    <Button onClick={ctx.withdrawAgain}>Try again</Button>
  </>
);

const STEPS: {
  [K in View['kind']]: (view: Extract<View, { kind: K }>, ctx: StepContext) => StepContent;
} = {
  paying: ({ destination, unknown }, ctx) => ({
    note: 'Lightning can take a few minutes. The payment continues if you close this.',
    footer: closeButton(ctx),
    body: (
      <DialogProgress
        steps={[
          { label: 'Starting the payment', done: true },
          unknown
            ? { label: "Can't check the payment right now", done: false }
            : { label: `Paying ${destination} over Lightning`, done: false }
        ]}
      />
    )
  }),
  sent: ({ destination, capped }, ctx) =>
    capped
      ? {
          footer: (
            <>
              <Button variant="secondary" onClick={ctx.close}>
                Done
              </Button>

              <Button onClick={ctx.withdrawAgain}>
                Withdraw the remaining {formatSats(capped.remaining_msat)}
              </Button>
            </>
          ),
          body: receipt(
            ctx,
            `Sent ${ctx.amount}`,
            `${destination} accepts at most ${formatSats(capped.maximum_msat)} per payment. ${formatSats(capped.remaining_msat)} are still in this balance.`
          )
        }
      : { body: receipt(ctx, `Sent ${ctx.amount}`, `to ${destination}`) },
  failed: (_view, ctx) => ({
    footer: retryFooter(ctx),
    body: receipt(
      ctx,
      'The payment did not go through',
      'Nothing left this host. The sats return to this balance when the federation finishes the refund.',
      'warning'
    )
  }),
  not_started: (_view, ctx) => ({
    footer: retryFooter(ctx),
    body: receipt(ctx, 'The withdrawal did not start', 'Nothing left this balance.', 'warning')
  }),
  code: ({ token }, ctx) => ({
    wide: true,
    footerStart: (
      <button type="button" className={styles.dangerLink} onClick={ctx.askCancel}>
        Cancel withdrawal
      </button>
    ),
    footer: (
      <>
        {closeButton(ctx)}

        <Button loading={ctx.confirm.pending} onClick={ctx.confirm.run}>
          I've redeemed it
        </Button>
      </>
    ),
    body: (
      <div className={styles.twoColumns}>
        <EcashQrCode token={token} />

        <div className={styles.codeText}>
          <p className={styles.codeHeading}>{ctx.amount} of ecash</p>

          <ol className={styles.steps}>
            <li>Open Fedi on your phone and tap Scan.</li>

            <li>Point it at this code.</li>

            <li>Wait until Fedi shows the payment.</li>
          </ol>

          <span className={styles.idRow}>
            <span className={styles.mono}>{truncateMiddle(token, 12, 8)}</span>
          </span>

          <div>
            <CopyButton value={token} label="Copy ecash" showLabel />
          </div>

          <div>
            <Chip tone="warn">Waiting to be redeemed</Chip>
          </div>

          <p className={styles.caption}>
            Anyone with this code can redeem it. Don't share your screen while it is open.
          </p>
          {ctx.failure}
        </div>
      </div>
    )
  }),
  confirm_cancel: (_view, ctx) => ({
    wide: true,
    onDismiss: ctx.keep,
    footer: (
      <>
        <Button variant="secondary" onClick={ctx.keep}>
          Keep ecash
        </Button>

        <Button variant="danger" onClick={ctx.cancel}>
          Cancel withdrawal
        </Button>
      </>
    ),
    body: (
      <div className={styles.confirm}>
        <p className={styles.codeHeading}>Cancel this withdrawal?</p>

        <p className={styles.body}>
          If the ecash has not been redeemed, the {ctx.amount} return to this balance and the code
          stops working. If it was already redeemed, nothing returns and we mark it redeemed.
        </p>
      </div>
    )
  }),
  // A cancel the federation has not settled. While this dialog's own cancel
  // runs, the step cannot be closed.
  returning: ({ committing }, ctx) => ({
    locked: committing,
    footer: committing ? null : closeButton(ctx),
    body: (
      <>
        <DialogProgress steps={[{ label: 'Checking with the federation…', done: false }]} />
        {ctx.failure}
      </>
    )
  }),
  redeemed: ({ confirmedAtMs }, ctx) => ({
    body: (
      <DialogResult tone="success" heading={`Redeemed ${ctx.amount}`}>
        <p>Marked by you · {formatLocalMoment(confirmedAtMs / 1000)}</p>

        <OperationId id={ctx.operationId} />

        <button type="button" className={styles.quietLink} onClick={ctx.askCancel}>
          Money didn't arrive? Take it back
        </button>
        {ctx.failure}
      </DialogResult>
    )
  }),
  returned: (_view, ctx) => ({
    body: receipt(ctx, `Returned ${ctx.amount}`, 'They are back in this balance.')
  }),
  already_redeemed: (_view, ctx) => ({
    body: receipt(
      ctx,
      'Already redeemed',
      'This ecash was redeemed before the cancel reached the federation, so nothing returned.'
    )
  })
};

const stepContent = <K extends View['kind']>(view: Extract<View, { kind: K }>, ctx: StepContext) =>
  (STEPS[view.kind] as (view: Extract<View, { kind: K }>, ctx: StepContext) => StepContent)(
    view,
    ctx
  );

/**
 * One payout, read from the daemon and shown as the step it has reached:
 * progress while it moves, the ecash code while the token is issued, and a
 * receipt once it is final. Everything shown comes from `PayoutStatus`, so a
 * payment reads "Sent" only once it succeeded. Every step but a running
 * cancel can be closed: the payout continues, and the page lists it.
 */
export const PayoutView = ({ requestId, onClose, onWithdrawAgain }: PayoutViewProps) => {
  const status = usePayoutStatus(requestId);
  const confirm = useConfirmEcashPayout(requestId);
  const cancel = useCancelEcashPayout(requestId);
  const [askingCancel, setAskingCancel] = useState(false);
  const payout = status.data;

  if (!payout) {
    return (
      <PayoutDialog
        title="Withdrawal"
        onDismiss={onClose}
        footer={
          <>
            {status.isError && (
              <Button variant="secondary" onClick={() => void status.refetch()}>
                Try again
              </Button>
            )}
            <Button variant="secondary" onClick={onClose}>
              Close
            </Button>
          </>
        }
      >
        {status.isError ? (
          <p role="alert" className={styles.error}>
            {describeActionError(status.error)}
          </p>
        ) : (
          <DialogProgress steps={[{ label: 'Reading the withdrawal…', done: false }]} />
        )}
      </PayoutDialog>
    );
  }

  const source = sourceOfScope(payout.job.scope);
  const failure = confirm.error ?? cancel.error;
  const content = stepContent(viewOf(payout, askingCancel, cancel.isPending), {
    amount: formatSats(payout.job.operation?.amount_msat),
    operationId: payout.job.operation?.operation_id ?? null,
    failure: failure && (
      <p role="alert" className={styles.error}>
        {describeActionError(failure)}
      </p>
    ),
    close: onClose,
    withdrawAgain: () => onWithdrawAgain(source),
    confirm: { run: () => confirm.mutate(), pending: confirm.isPending },
    askCancel: () => setAskingCancel(true),
    keep: () => setAskingCancel(false),
    cancel: () => {
      setAskingCancel(false);
      cancel.mutate(payout.job.scope);
    }
  });

  return (
    <PayoutDialog
      title={withdrawTitle(source)}
      source={source}
      wide={content.wide}
      onDismiss={content.locked ? undefined : (content.onDismiss ?? onClose)}
      note={content.note}
      footerStart={content.footerStart}
      footer={
        content.footer === undefined ? <Button onClick={onClose}>Done</Button> : content.footer
      }
    >
      {content.body}
    </PayoutDialog>
  );
};
