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

      <Link to="/offer" className={styles.editLink}>
        Change price
      </Link>
    </div>
  </SectionCard>
);
