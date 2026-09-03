import { SectionCard } from '@operator-ui/common-ui';
import { Link } from 'react-router-dom';
import { describeOffer } from '@/shared/utils/offerPrice';
import styles from './OfferSummary.module.css';

interface OfferSummaryProps {
  priceMsat: number | null;
}

export const OfferSummary = ({ priceMsat }: OfferSummaryProps) => (
  <SectionCard title="Your offer">
    <div className={styles.root}>
      <span className={styles.price}>{describeOffer(priceMsat)}</span>

      {/* The page behind this link sets the seat ceiling as well as the price,
          and it is the only way to reach either. A link that named only the
          price is how the ceiling stayed unreachable after setup. */}
      <Link to="/offer" className={styles.editLink}>
        Change offer
      </Link>
    </div>
  </SectionCard>
);
