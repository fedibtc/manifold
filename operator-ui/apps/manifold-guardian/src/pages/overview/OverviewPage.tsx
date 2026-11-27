import { Banner, StatCard } from '@operator-ui/common-ui';
import { useSeatGrants } from '@/features/guardian-codes/api/hooks/use-seat-grants/useSeatGrants';
import { AttentionList } from '@/features/overview/components/attention-list/AttentionList';
import { EarningsTimeline } from '@/features/overview/components/earnings-timeline/EarningsTimeline';
import { SeatAvailability } from '@/features/overview/components/seat-availability/SeatAvailability';
import { useOverviewEarnings } from '@/features/overview/hooks/use-overview-earnings/useOverviewEarnings';
import { deriveOverview, WALLET_RECOVERING } from '@/features/overview/utils/deriveOverview';
import { useMobileNotificationsNews } from '@/features/phone-link/hooks/use-mobile-notifications-news/useMobileNotificationsNews';
import { useOffer } from '@/shared/api/hooks/use-offer/useOffer';
import { useOnboarding } from '@/shared/api/hooks/use-onboarding/useOnboarding';
import { usePaymentFederations } from '@/shared/api/hooks/use-payment-federations/usePaymentFederations';
import { useSeatReadiness } from '@/shared/api/hooks/use-seat-readiness/useSeatReadiness';
import { useSeats } from '@/shared/api/hooks/use-seats/useSeats';
import { QuerySurface } from '@/shared/components/query-surface/QuerySurface';
import { WhatsNew } from '@/shared/components/whats-new/WhatsNew';
import { useQueryDisposition } from '@/shared/query/use-query-disposition/useQueryDisposition';
import { guardianCodesAvailable, isAuthorized, isBadgeless } from '@/shared/utils/authorization';
import { formatSats } from '@/shared/utils/format';
import { readOfferPriceMsat } from '@/shared/utils/offerPrice';
import styles from './OverviewPage.module.css';

const toneVariant = { success: 'success', warn: 'warn' } as const;

export const OverviewPage = () => {
  const seats = useSeats();
  // Announcements for the "What's new" slot, newest first.
  const whatsNew = [useMobileNotificationsNews()].filter((item) => item !== null);
  const paymentFederations = usePaymentFederations();
  const offer = useOffer();
  const earnings = useOverviewEarnings();
  // Deliberately outside the disposition below: the Overview must still render
  // when the authorization state is unknown.
  const onboarding = useOnboarding();
  // Outside the disposition for the same reason.
  const seatReadiness = useSeatReadiness();
  const { disposition: readiness } = useQueryDisposition([seatReadiness]);
  const readinessUnavailable = readiness.kind === 'failed' || readiness.kind === 'stale';

  // The three fleet-wide reads behind every figure on this page. A failure while
  // they hold answers marks the page stale — it never deletes the figures, which
  // would leave the operator blank for the whole outage rather than one render.
  const { disposition, retry } = useQueryDisposition([seats, paymentFederations, offer]);

  const plans = offer.data?.plans ?? [];
  const badgeless = isBadgeless(onboarding.data);
  const guardianCodes = guardianCodesAvailable(onboarding.data);
  // Outside the disposition too: the count is one line of the availability
  // card, not a figure the page cannot render without.
  const seatGrants = useSeatGrants(guardianCodes);
  const unusedCodes = seatGrants.data?.grants.filter(
    (grant) => grant.status.state === 'unused'
  ).length;
  const model = deriveOverview({
    badgeless,
    paymentFederations: paymentFederations.data?.federations,
    plans,
    nostrState: onboarding.data?.nostr.state,
    seatReadiness: seatReadiness.data
  });
  const recovering = (paymentFederations.data?.federations ?? []).some(
    (federation) => federation.wallet.recovering
  );
  const unreadableFees =
    earnings.unreadableFeeSeatCount > 0
      ? `Guardian fees for ${earnings.unreadableFeeSeatCount} seat(s) couldn't be read and aren't included above.`
      : null;

  return (
    <div className={styles.root}>
      <h1 className={styles.heading}>Overview</h1>

      <WhatsNew items={whatsNew} />

      <QuerySurface disposition={disposition} onRetry={retry}>
        <Banner variant={readinessUnavailable ? 'warn' : toneVariant[model.tone]}>
          {readinessUnavailable ? 'Readiness unavailable' : model.headline}
        </Banner>

        <div className={styles.tileGrid}>
          <StatCard
            label="Held in federations"
            value={formatSats(earnings.balanceMsat)}
            hint={recovering ? `${WALLET_RECOVERING}.` : undefined}
          />

          <StatCard
            label="Earned, all time"
            value={formatSats(earnings.totalMsat)}
            hint="*Network fees apply"
          />

          <StatCard label="Seat sales, all time" value={formatSats(earnings.seatSalesMsat)} />

          <StatCard label="Guardian fees, all time" value={formatSats(earnings.guardianFeesMsat)} />
        </div>

        <SeatAvailability
          priceMsat={readOfferPriceMsat(plans)}
          guardianCodes={guardianCodes}
          authorized={isAuthorized(onboarding.data)}
          unusedCodes={unusedCodes}
        />

        <AttentionList items={model.attention} />

        <EarningsTimeline days={earnings.days} />

        <ul className={styles.caveats}>
          <li>Amounts shown are what buyers paid. Network fees apply.</li>

          <li>A seat sale is counted once the buyer's payment has completed.</li>

          {/* The price is frozen on the seat at quote time and the seats table
              is immutable, so a sale can differ from the price above without
              either figure being wrong. */}
          <li>Each sale is counted at the price it sold for, not your current seat price.</li>

          {unreadableFees && <li>{unreadableFees}</li>}
        </ul>
      </QuerySurface>
    </div>
  );
};
