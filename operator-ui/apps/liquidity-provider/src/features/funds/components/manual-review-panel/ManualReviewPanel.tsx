import { Banner, Button, SectionCard, SelectField, TextInput } from '@operator-ui/common-ui';
import type { ManualReviewResolution, WalletOperation } from '@operator-ui/types';
import { type KeyboardEvent, useEffect, useRef, useState } from 'react';
import { useWalletOperation } from '@/features/funds/api/hooks/use-wallet-operation/useWalletOperation';
import { useResolveManualReview } from '@/features/funds/hooks/use-resolve-manual-review/useResolveManualReview';
import { describeActionError } from '@/shared/utils/describeActionError';
import { formatDateTime, formatSats, UNKNOWN_AMOUNT } from '@/shared/utils/format';
import styles from './ManualReviewPanel.module.css';

const RESOLUTION_OPTIONS = [
  { value: 'completed', label: 'Completed — the send settled on chain' },
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
 * The three outcomes are the daemon's, not this screen's invention, and the
 * transaction id is mandatory for `completed` because that resolution asserts a
 * specific on-chain settlement — the daemon rejects it without one, and rejects
 * one supplied with either other outcome, which assert that no send happened.
 */
export const ManualReviewPanel = ({ operationId, onClose }: ManualReviewPanelProps) => {
  const operation = useWalletOperation(operationId);
  const resolve = useResolveManualReview();
  const [resolution, setResolution] = useState<ManualReviewResolution>('safe_to_retry');
  const [txid, setTxid] = useState('');
  const [reason, setReason] = useState('');
  const [txidError, setTxidError] = useState<string | null>(null);
  const panelRef = useRef<HTMLDivElement>(null);

  // Opening this replaces the row's control, so focus has to land somewhere
  // named. Same reasoning as WithdrawConfirm.
  useEffect(() => {
    panelRef.current?.focus();
  }, []);

  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== 'Escape' || resolve.isPending) return;
    event.stopPropagation();
    onClose();
  };

  const handleSubmit = () => {
    const trimmedTxid = txid.trim();
    if (resolution === 'completed' && trimmedTxid.length === 0) {
      setTxidError('Enter the transaction that settled this send.');
      return;
    }
    setTxidError(null);
    const trimmedReason = reason.trim();
    resolve.mutate(
      {
        operation_id: operationId,
        resolution,
        txid: resolution === 'completed' ? trimmedTxid : null,
        reason: trimmedReason.length > 0 ? trimmedReason : null
      },
      { onSuccess: onClose }
    );
  };

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
          onChange={(value) => setResolution(value as ManualReviewResolution)}
          options={RESOLUTION_OPTIONS}
          disabled={resolve.isPending}
        />

        {resolution === 'completed' && (
          <TextInput
            label="Transaction id"
            value={txid}
            onChange={setTxid}
            error={txidError ?? undefined}
            hint="The on-chain transaction that settled this send."
            disabled={resolve.isPending}
          />
        )}

        <TextInput
          label="Reason (optional)"
          value={reason}
          onChange={setReason}
          // The admin API authenticates one shared bearer token, so the daemon
          // cannot record who resolved this. The audit row keeps the reason
          // verbatim, which makes it the only place a name can go.
          hint="Recorded in the audit log. Name yourself here — the daemon cannot tell operators apart."
          disabled={resolve.isPending}
        />

        {resolve.isError && (
          <Banner variant="error" title="Couldn't resolve">
            {describeActionError(resolve.error)}
          </Banner>
        )}

        <div className={styles.actions}>
          <Button variant="primary" size="small" loading={resolve.isPending} onClick={handleSubmit}>
            Resolve
          </Button>

          <Button variant="secondary" size="small" disabled={resolve.isPending} onClick={onClose}>
            Cancel
          </Button>
        </div>
      </SectionCard>
    </div>
  );
};
