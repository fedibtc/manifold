import { Banner, Button, SectionCard, SelectField, TextInput } from '@operator-ui/common-ui';
import type {
  ManualOperationStatus,
  ManualReviewResolution,
  WalletOperation
} from '@operator-ui/types';
import { type KeyboardEvent, type ReactNode, useEffect, useRef, useState } from 'react';
import { useWalletOperation } from '@/features/funds/api/hooks/use-wallet-operation/useWalletOperation';
import { useCompleteReviewWithoutEvidence } from '@/features/funds/hooks/use-complete-review-without-evidence/useCompleteReviewWithoutEvidence';
import { useResolveManualReview } from '@/features/funds/hooks/use-resolve-manual-review/useResolveManualReview';
import { describeActionError } from '@/shared/utils/describeActionError';
import { formatDateTime, formatSats, UNKNOWN_AMOUNT } from '@/shared/utils/format';
import styles from './ManualReviewPanel.module.css';

// The fourth entry is not a fourth daemon resolution. `completed`, `failed`,
// and `safe_to_retry` go to `resolve_manual_review`; this one goes to
// `complete_review_without_evidence`, which is a separate verb. They share one
// control because an operator picks between them by what they know about the
// send, not by which endpoint answers.
const ASSERTED = 'completed_without_evidence';

type Outcome = ManualReviewResolution | typeof ASSERTED;

const RESOLUTION_OPTIONS = [
  { value: 'completed', label: 'Completed — the send settled on chain' },
  {
    value: ASSERTED,
    label: 'Completed without chain evidence — asserted, not verified'
  },
  { value: 'failed', label: 'Failed — no send happened, do not retry' },
  { value: 'safe_to_retry', label: 'Safe to retry — no send happened, retry is allowed' }
];

// What the daemon holds as evidence about a send it could not settle. Rendered
// as facts, never as an absence dressed up as one: a missing txid prints as
// unknown rather than as a blank, because "we have no transaction" is the whole
// question the operator is here to answer.
const evidenceRows = (operation: WalletOperation): [string, string][] => [
  ['Amount', formatSats(operation.amount)],
  ['Destination', operation.address ?? UNKNOWN_AMOUNT],
  ['Transaction', operation.txid ?? 'None recorded'],
  [
    'Output index',
    operation.tx_vout === null || operation.tx_vout === undefined
      ? 'None recorded'
      : String(operation.tx_vout)
  ],
  [
    'Confirmations',
    operation.confirmation_count === null || operation.confirmation_count === undefined
      ? 'None observed'
      : String(operation.confirmation_count)
  ],
  ['Requested', formatDateTime(operation.created_at)],
  ['Last change', formatDateTime(operation.updated_at)],
  ['Failure', operation.failure?.message ?? 'None recorded']
];

interface ManualReviewPanelProps {
  operationId: string;
  onClose: () => void;
}

/**
 * The only exit from manual review, inside the product.
 *
 * A send whose gateway reply was lost is recorded in doubt, escalated to manual
 * review after the configured wait, and then frozen: the sync pass skips it and
 * retry refuses it, so nothing but an operator's judgement moves it. Until this
 * screen existed the daemon had the route and the dashboard had no client for
 * it, so four screens printed "needs review" with nothing to click and the money
 * sat there until somebody reached for a command line.
 *
 * The outcomes are the daemon's, not this screen's invention, and the
 * transaction id is mandatory for `completed` because that resolution asserts a
 * specific on-chain settlement — the daemon rejects it without one, and rejects
 * one supplied with either no-send outcome.
 *
 * `completed` also requires chain evidence that the transaction pays this
 * operation's exact destination and amount, which the daemon checks itself.
 * A send whose outcome the operator established off chain cannot satisfy that
 * and exits through `complete_review_without_evidence` instead: same
 * completion, recorded as asserted rather than verified, with a mandatory
 * reason because the audit row is the only thing that marks the difference.
 */
export const ManualReviewPanel = ({ operationId, onClose }: ManualReviewPanelProps) => {
  const operation = useWalletOperation(operationId);
  const resolve = useResolveManualReview();
  const completeWithoutEvidence = useCompleteReviewWithoutEvidence();
  const [resolution, setResolution] = useState<Outcome>('safe_to_retry');
  const [txid, setTxid] = useState('');
  const [reason, setReason] = useState('');
  const [txidError, setTxidError] = useState<string | null>(null);
  const [reasonError, setReasonError] = useState<string | null>(null);
  const panelRef = useRef<HTMLDivElement>(null);

  const asserted = resolution === ASSERTED;
  // Both routes end this review, so either one pending has to lock the form.
  const pending = resolve.isPending || completeWithoutEvidence.isPending;
  const needsTxid = resolution === 'completed' || asserted;

  // Opening this replaces the row's control, so focus has to land somewhere
  // named. Same reasoning as WithdrawConfirm.
  useEffect(() => {
    panelRef.current?.focus();
  }, []);

  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== 'Escape' || pending) return;
    event.stopPropagation();
    onClose();
  };

  const handleSubmit = () => {
    const trimmedTxid = txid.trim();
    const trimmedReason = reason.trim();

    // Validate both fields before returning, so an operator who left both
    // blank is told both times rather than once per attempt.
    const missingTxid = needsTxid && trimmedTxid.length === 0;
    const missingReason = asserted && trimmedReason.length === 0;
    setTxidError(
      missingTxid
        ? asserted
          ? 'Enter the transaction you are asserting settled this send.'
          : 'Enter the transaction that settled this send.'
        : null
    );
    setReasonError(
      missingReason ? 'Required: record how you established this send settled.' : null
    );
    if (missingTxid || missingReason) return;

    // Both verbs answer a refusal with a status on an otherwise successful
    // call, so the review is over only on `accepted`. Closing on delivery
    // instead would drop the operator back to a row still marked for review
    // with nothing saying why.
    const closeIfAccepted = {
      onSuccess: (response: { status: ManualOperationStatus }) => {
        if (response.status === 'accepted') onClose();
      }
    };

    if (asserted) {
      completeWithoutEvidence.mutate(
        { operation_id: operationId, txid: trimmedTxid, reason: trimmedReason },
        closeIfAccepted
      );
      return;
    }

    resolve.mutate(
      {
        operation_id: operationId,
        resolution,
        txid: resolution === 'completed' ? trimmedTxid : null,
        reason: trimmedReason.length > 0 ? trimmedReason : null
      },
      closeIfAccepted
    );
  };

  // The daemon answers both verbs with a status rather than an error when it
  // declines, so a rejection has to be read off the response body. Same shape
  // as AllocationTimeline's retry and cancel banners.
  let outcomeBanner: ReactNode = null;
  if (completeWithoutEvidence.isError) {
    outcomeBanner = (
      <Banner variant="error" title="Couldn't complete">
        {describeActionError(completeWithoutEvidence.error)}
      </Banner>
    );
  } else if (resolve.isError) {
    outcomeBanner = (
      <Banner variant="error" title="Couldn't resolve">
        {describeActionError(resolve.error)}
      </Banner>
    );
  } else if (completeWithoutEvidence.data && completeWithoutEvidence.data.status !== 'accepted') {
    outcomeBanner = (
      <Banner variant="error" title="Completion not applied">
        {completeWithoutEvidence.data.detail ?? 'The daemon could not complete this review.'}
      </Banner>
    );
  } else if (resolve.data && resolve.data.status !== 'accepted') {
    outcomeBanner = (
      <Banner variant="error" title="Resolution not applied">
        {resolve.data.detail ?? 'The daemon could not resolve this review.'}
      </Banner>
    );
  }

  return (
    // A prompt that names itself, not a group of related fields: see the same
    // note on WithdrawConfirm. tabIndex -1 is programmatic focus, not a stop.
    // biome-ignore lint/a11y/useSemanticElements: see the note above
    <div
      ref={panelRef}
      role="group"
      aria-label="Resolve manual review"
      tabIndex={-1}
      onKeyDown={handleKeyDown}
      className={styles.panel}
    >
      <SectionCard title="Resolve manual review">
        {operation.isError && (
          <Banner variant="error" title="Couldn't load the operation">
            {describeActionError(operation.error)}
          </Banner>
        )}

        {!operation.data && !operation.isError && <p className={styles.state}>Loading…</p>}

        {operation.data && (
          <dl className={styles.evidence}>
            {evidenceRows(operation.data.operation).map(([label, value]) => (
              <div key={label} className={styles.evidenceRow}>
                <dt className={styles.evidenceLabel}>{label}</dt>

                <dd className={styles.evidenceValue}>{value}</dd>
              </div>
            ))}
          </dl>
        )}

        <SelectField
          label="Outcome"
          value={resolution}
          onChange={(value) => setResolution(value as Outcome)}
          options={RESOLUTION_OPTIONS}
          disabled={pending}
        />

        {asserted && (
          <Banner variant="warn" title="FLIP will not verify this">
            This records the send as completed on your assertion alone. FLIP does not check that the
            transaction pays this operation's destination and amount, and the completion is final.
            Use it only where you have established the outcome another way.
          </Banner>
        )}

        {needsTxid && (
          <TextInput
            label="Transaction id"
            value={txid}
            onChange={setTxid}
            error={txidError ?? undefined}
            hint={
              asserted
                ? 'Stored as asserted. FLIP records it without checking that it pays this send.'
                : 'The on-chain transaction that settled this send.'
            }
            disabled={pending}
          />
        )}

        <TextInput
          label={asserted ? 'Reason' : 'Reason (optional)'}
          value={reason}
          onChange={setReason}
          error={reasonError ?? undefined}
          // The admin API authenticates one shared bearer token, so the daemon
          // cannot record who resolved this. The audit row keeps the reason
          // verbatim, which makes it the only place a name can go.
          hint={
            asserted
              ? 'Recorded in the audit log, which is the only record that this completion was asserted. Say how you established the outcome, and name yourself — the daemon cannot tell operators apart.'
              : 'Recorded in the audit log. Name yourself here — the daemon cannot tell operators apart.'
          }
          disabled={pending}
        />

        {outcomeBanner}

        <div className={styles.actions}>
          <Button variant="primary" size="small" loading={pending} onClick={handleSubmit}>
            {asserted ? 'Complete without evidence' : 'Resolve'}
          </Button>

          <Button variant="secondary" size="small" disabled={pending} onClick={onClose}>
            Cancel
          </Button>
        </div>
      </SectionCard>
    </div>
  );
};
