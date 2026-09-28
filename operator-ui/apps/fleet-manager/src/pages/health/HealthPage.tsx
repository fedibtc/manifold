import { Banner } from '@operator-ui/common-ui';
import { Link } from 'react-router-dom';
import { AttentionList } from '@/features/overview/components/attention-list/AttentionList';
import { deriveOverview } from '@/features/overview/utils/deriveOverview';
import { SelfCheckCard } from '@/features/self-check/SelfCheckCard';
import { useOffer } from '@/shared/api/hooks/use-offer/useOffer';
import { useOnboarding } from '@/shared/api/hooks/use-onboarding/useOnboarding';
import { usePaymentFederations } from '@/shared/api/hooks/use-payment-federations/usePaymentFederations';
import { QuerySurface } from '@/shared/components/query-surface/QuerySurface';
import { useQueryDisposition } from '@/shared/query/use-query-disposition/useQueryDisposition';
import styles from './HealthPage.module.css';

export const HealthPage = () => {
  const paymentFederations = usePaymentFederations();
  const offer = useOffer();
  const onboarding = useOnboarding();
  const { disposition, retry } = useQueryDisposition([paymentFederations, offer, onboarding]);
  const model = deriveOverview({
    paymentFederations: paymentFederations.data?.federations,
    plans: offer.data?.plans,
    nostrState: onboarding.data?.nostr.state
  });

  return (
    <div className={styles.root}>
      <h1 className={styles.heading}>Health</h1>

      <section className={styles.standard} aria-label="Fleet health">
        <h2>Fleet status</h2>

        <QuerySurface disposition={disposition} onRetry={retry}>
          <Banner variant={model.tone}>{model.headline}</Banner>

          <AttentionList items={model.attention} />

          <p>
            For individual guardian status, see <Link to="/seats">Seats</Link>.
          </p>
        </QuerySurface>
      </section>

      <SelfCheckCard />
    </div>
  );
};
