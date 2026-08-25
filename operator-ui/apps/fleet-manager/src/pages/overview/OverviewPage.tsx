import { Banner, StatCard } from '@operator-ui/common-ui';
import { AttentionList } from '@/features/overview/components/attention-list/AttentionList';
import { EarningsTimeline } from '@/features/overview/components/earnings-timeline/EarningsTimeline';
import { OfferSummary } from '@/features/overview/components/offer-summary/OfferSummary';
import { useOverviewEarnings } from '@/features/overview/hooks/use-overview-earnings/useOverviewEarnings';
import { deriveOverview } from '@/features/overview/utils/deriveOverview';
import { useOffer } from '@/shared/api/hooks/use-offer/useOffer';
import { useOnboarding } from '@/shared/api/hooks/use-onboarding/useOnboarding';
import { usePaymentFederations } from '@/shared/api/hooks/use-payment-federations/usePaymentFederations';
import { useSeats } from '@/shared/api/hooks/use-seats/useSeats';
import { QuerySurface } from '@/shared/components/query-surface/QuerySurface';
import { useQueryDisposition } from '@/shared/query/use-query-disposition/useQueryDisposition';
import { formatSats } from '@/shared/utils/format';
import { readOfferPriceMsat } from '@/shared/utils/offerPrice';
import styles from './OverviewPage.module.css';

const toneVariant = { success: 'success', warn: 'warn' } as const;

export const OverviewPage = () => {
  const seats = useSeats();
  const paymentFederations = usePaymentFederations();
  const offer = useOffer();
  const earnings = useOverviewEarnings();
  // Deliberately outside the disposition below: the Overview must still render
  // when the authorization state is unknown.
  const onboarding = useOnboarding();

  // The three fleet-wide reads behind every figure on this page. A failure while
  // they hold answers marks the page stale — it never deletes the figures, which
  // would leave the operator blank for the whole outage rather than one render.
  const { disposition, retry } = useQueryDisposition([seats, paymentFederations, offer]);

  const plans = offer.data?.plans ?? [];
  const model = deriveOverview({
    paymentFederations: paymentFederations.data?.federations,
    plans,
    nostrState: onboarding.data?.nostr.state
  });
  const unreadableFees =
    earnings.unreadableFeeSeatCount > 0
      ? ` Fee revenue could not be read for ${earnings.unreadableFeeSeatCount} seat(s), so none is counted for them.`
      : '';

  return (
    <div className={styles.root}>
      <h1 className={styles.heading}>Overview</h1>

      <QuerySurface disposition={disposition} onRetry={retry}>
        <Banner variant={toneVariant[model.tone]}>{model.headline}</Banner>

        <div className={styles.tileGrid}>
          <StatCard label="Wallet balance" value={formatSats(earnings.balanceMsat)} />

          <StatCard
            label="Earned, all time"
            value={formatSats(earnings.totalMsat)}
            hint="Gross, before fees"
          />

          <StatCard label="Seat sales" value={formatSats(earnings.seatSalesMsat)} />

          <StatCard label="Guardian fees" value={formatSats(earnings.guardianFeesMsat)} />
        </div>

        <OfferSummary priceMsat={readOfferPriceMsat(plans)} />

        <AttentionList items={model.attention} />

        <EarningsTimeline days={earnings.days} />

        <p className={styles.caveats}>
          Every figure here is <strong>gross</strong> — what the fleet was paid, before the mint and
          Lightning fees taken on the way. Seat sales count <strong>accepted payment claims</strong>
          , which is not the same as a settled payment.{unreadableFees}
        </p>
      </QuerySurface>
    </div>
  );
};
