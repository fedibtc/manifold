import { Banner, Button, Chip, type ChipTone } from '@operator-ui/common-ui';
import type { CreateDepositAddressResponse, WalletOperationStatus } from '@operator-ui/types';
import { QRCodeSVG } from 'qrcode.react';
import { useEffect, useRef, useState } from 'react';
import { useWalletOperations } from '@/features/funds/api/hooks/use-wallet-operations/useWalletOperations';
import { useCreateDepositAddress } from '@/features/funds/hooks/use-create-deposit-address/useCreateDepositAddress';
import { describeActionError } from '@/features/funds/utils/describeActionError';
import { formatSats } from '@/shared/utils/format';
import styles from './TopupPanel.module.css';
import { useCopyAddress } from './useCopyAddress';

const QR_SIZE = 168;

const WATCH_LABELS: Record<WalletOperationStatus, string> = {
  pending: 'Waiting for deposit',
  broadcast: 'Broadcast',
  confirmed: 'Confirmed',
  completed: 'Completed',
  in_doubt: 'Needs review',
  manual_review_required: 'Needs review',
  failed: 'Failed',
  cancelled: 'Cancelled'
};

const WATCH_TONES: Record<WalletOperationStatus, ChipTone> = {
  pending: 'warn',
  broadcast: 'info',
  confirmed: 'info',
  completed: 'ok',
  in_doubt: 'warn',
  manual_review_required: 'warn',
  failed: 'bad',
  cancelled: 'neutral'
};

const COPY_LABELS = {
  idle: 'Copy address',
  copied: 'Copied',
  selected: 'Select and copy'
} as const;

export const TopupPanel = () => {
  const deposit = useCreateDepositAddress();
  const { mutate: createAddress } = deposit;
  const addressRef = useRef<HTMLParagraphElement>(null);
  const { state: copyState, copy } = useCopyAddress();
  const [displayedDeposit, setDisplayedDeposit] = useState<CreateDepositAddressResponse | null>(
    null
  );

  useEffect(() => {
    createAddress();
  }, [createAddress]);

  // A "New address" retry clears the mutation's `data` while it is in flight —
  // keep the last-good address + QR on screen until the new one arrives. Adjust
  // state during render (guarded) rather than in an effect, per React guidance.
  if (deposit.data && deposit.data !== displayedDeposit) {
    setDisplayedDeposit(deposit.data);
  }

  const operationId = displayedDeposit?.operation_id ?? null;

  const walletOperations = useWalletOperations({ watch: operationId !== null });
  const items = walletOperations.data?.operations.items ?? [];
  const watchedOp = operationId ? items.find((op) => op.operation_id === operationId) : undefined;
  const watchedStatus = watchedOp?.status ?? 'pending';

  const handleNewAddress = () => createAddress();
  const handleCopy = () => {
    if (displayedDeposit) void copy(displayedDeposit.address, addressRef.current);
  };

  if (deposit.isError && !displayedDeposit) {
    return (
      <div className={styles.root}>
        <Banner
          variant="error"
          title="Couldn't create a deposit address"
          action={
            <Button variant="secondary" size="small" onClick={handleNewAddress}>
              Retry
            </Button>
          }
        >
          {describeActionError(deposit.error)}
        </Banner>
      </div>
    );
  }

  if (!displayedDeposit) {
    return (
      <div className={styles.root}>
        <p className={styles.loading}>Requesting a deposit address…</p>
      </div>
    );
  }

  return (
    <div className={styles.root}>
      {deposit.isError && (
        <Banner
          variant="error"
          title="Couldn't create a new address"
          action={
            <Button variant="secondary" size="small" onClick={handleNewAddress}>
              Retry
            </Button>
          }
        >
          {describeActionError(deposit.error)}
        </Banner>
      )}

      <div className={styles.top}>
        <div className={styles.qr}>
          <QRCodeSVG value={displayedDeposit.address} size={QR_SIZE} marginSize={2} />
        </div>

        <div className={styles.details}>
          <p ref={addressRef} className={styles.address}>
            {displayedDeposit.address}
          </p>

          <div className={styles.actions}>
            <Button variant="secondary" size="small" onClick={handleCopy}>
              {COPY_LABELS[copyState]}
            </Button>

            <Button
              variant="secondary"
              size="small"
              onClick={handleNewAddress}
              disabled={deposit.isPending}
            >
              New address
            </Button>
          </div>

          <p className={styles.network}>
            <strong className={styles.networkName}>{displayedDeposit.network}</strong> address.
            Funds sent on another network won't arrive.
          </p>
        </div>
      </div>

      <Banner variant="info" title="Watching for your deposit">
        It will appear under Wallet operations as pending, then count toward your balance after 3
        confirmations.
      </Banner>

      {operationId && (
        <section className={styles.watch}>
          <h3 className={styles.watchHeading}>This top-up</h3>

          <div className={styles.watchRow}>
            <span className={styles.watchId}>{operationId}</span>

            <span className={styles.watchAmount}>
              {watchedOp ? formatSats(watchedOp.amount) : '—'}
            </span>

            <Chip tone={WATCH_TONES[watchedStatus]}>{WATCH_LABELS[watchedStatus]}</Chip>
          </div>
        </section>
      )}
    </div>
  );
};
