import { Banner, Button, StaleDataBanner } from '@operator-ui/common-ui';
import type { AdvertisementPublicationStatus } from '@operator-ui/types';
import { useEffect, useRef, useState } from 'react';
import { AdvertisementHeader } from '@/features/advertisement/components/advertisement-header/AdvertisementHeader';
import { ListingCard } from '@/features/advertisement/components/listing-card/ListingCard';
import { RelaysTable } from '@/features/advertisement/components/relays-table/RelaysTable';
import {
  type ChipTone,
  StatusChip
} from '@/features/advertisement/components/status-chip/StatusChip';
import { WithdrawConfirm } from '@/features/advertisement/components/withdraw-confirm/WithdrawConfirm';
import { useAdvertisementState } from '@/features/advertisement/hooks/use-advertisement-state/useAdvertisementState';
import { useNow } from '@/features/advertisement/hooks/use-now/useNow';
import { useRefreshRelays } from '@/features/advertisement/hooks/use-refresh-relays/useRefreshRelays';
import { useRepublishAdvertisement } from '@/features/advertisement/hooks/use-republish-advertisement/useRepublishAdvertisement';
import { useWithdrawAdvertisement } from '@/features/advertisement/hooks/use-withdraw-advertisement/useWithdrawAdvertisement';
import { deriveAdvertisement } from '@/features/advertisement/services/format';
import styles from './AdvertisementPage.module.css';

const PUBLICATION_TONE: Record<AdvertisementPublicationStatus, ChipTone> = {
  not_ready: 'warn',
  published: 'ok',
  stale: 'warn',
  withdrawn: 'neutral',
  failed: 'bad'
};

const PUBLICATION_LABEL: Record<AdvertisementPublicationStatus, string> = {
  not_ready: 'Not ready',
  published: 'Published',
  stale: 'Stale',
  withdrawn: 'Withdrawn',
  failed: 'Failed'
};

export const AdvertisementPage = () => {
  const { data, isError, dataUpdatedAt } = useAdvertisementState();
  const republish = useRepublishAdvertisement();
  const withdraw = useWithdrawAdvertisement();
  const refresh = useRefreshRelays();
  const now = useNow();
  const [showWithdrawConfirm, setShowWithdrawConfirm] = useState(false);

  // WithdrawConfirm takes focus when it opens; closing it must hand focus back
  // to the control that opened it, or the operator is left with focus on
  // nothing. Mirrors the was-gated pattern in useBootStatus.
  const withdrawTriggerRef = useRef<HTMLButtonElement>(null);
  const wasConfirming = useRef(false);
  useEffect(() => {
    if (wasConfirming.current && !showWithdrawConfirm) withdrawTriggerRef.current?.focus();
    wasConfirming.current = showWithdrawConfirm;
  }, [showWithdrawConfirm]);

  const handleRepublish = () => republish.mutate();
  const handleWithdraw = () => setShowWithdrawConfirm(true);
  const handleWithdrawCancel = () => setShowWithdrawConfirm(false);
  const handleWithdrawConfirm = (reason: string | null) =>
    withdraw.mutate(reason, { onSuccess: () => setShowWithdrawConfirm(false) });
  const handleRefresh = () => refresh.mutate();

  // The page polls every 30s, so a single failed refetch must not wipe a
  // working screen: the error branch is for having no state at all, and a
  // failure on top of cached state keeps the listing visible under a stale
  // banner instead.
  if (!data) {
    if (isError) {
      return (
        <div className={styles.root}>
          <AdvertisementHeader />

          <Banner variant="error" title="Couldn't load advertisement state">
            Retry once the daemon is reachable.
          </Banner>
        </div>
      );
    }

    return (
      <div className={styles.root}>
        <AdvertisementHeader />

        <p className={styles.muted}>Loading advertisement state…</p>
      </div>
    );
  }

  const view = deriveAdvertisement(data, now);
  const notReady = data.publication_status === 'not_ready';
  const blockingReasons = notReady
    ? (data.readiness?.checks ?? [])
        .filter((check) => check.status === 'failed' && check.detail)
        .map((check) => check.detail as string)
    : [];

  return (
    <div className={styles.root}>
      <AdvertisementHeader
        status={
          <StatusChip tone={PUBLICATION_TONE[data.publication_status]}>
            {PUBLICATION_LABEL[data.publication_status]}
          </StatusChip>
        }
      />

      {isError && <StaleDataBanner updatedAtMs={dataUpdatedAt} />}

      {view.withdrawnAt && (
        <Banner variant="info" title={`Withdrawn by you at ${view.withdrawnAt} UTC`}>
          You are off the relays and stay off them until you republish. Republish now is the way
          back.
        </Banner>
      )}

      <ListingCard
        provider={view.provider}
        lastPublished={view.lastPublished}
        expires={view.expires}
        sources={view.sources}
        endpoint={view.endpoint}
      />

      <div className={styles.relayActions}>
        {/*
          Named for what it does. This verb republishes the advertisement to
          every relay; it does not merely re-read their state, and "Refresh"
          read as though it did. It respects a withdrawal, so it is disabled
          while the operator is off the market — Republish now is the only way
          back, and two controls that both publish would hide which one
          overrides the withdrawal.
        */}
        <Button
          variant="secondary"
          onClick={handleRefresh}
          loading={refresh.isPending}
          disabled={republish.isPending || withdraw.isPending || view.isWithdrawn}
        >
          Resend to relays
        </Button>
      </div>

      <RelaysTable relays={data.relay_states} now={now} />

      {blockingReasons.length > 0 && (
        <Banner variant="warn" title="Not ready to publish">
          <ul className={styles.blockingReasons}>
            {blockingReasons.map((reason) => (
              <li key={reason}>{reason}</li>
            ))}
          </ul>
        </Banner>
      )}

      <div className={styles.actions}>
        <Button
          onClick={handleRepublish}
          loading={republish.isPending}
          disabled={withdraw.isPending || notReady}
        >
          Republish now
        </Button>

        {!showWithdrawConfirm && (
          <Button
            ref={withdrawTriggerRef}
            variant="secondary"
            onClick={handleWithdraw}
            loading={withdraw.isPending}
            disabled={view.isWithdrawn || republish.isPending}
          >
            Withdraw advertisement
          </Button>
        )}
      </div>

      {showWithdrawConfirm && (
        <WithdrawConfirm
          onConfirm={handleWithdrawConfirm}
          onCancel={handleWithdrawCancel}
          isPending={withdraw.isPending}
        />
      )}

      <p className={styles.note}>
        Withdrawing hides you from every app until you republish — nothing puts you back on the
        relays by itself. You'll be asked to confirm.
      </p>
    </div>
  );
};
