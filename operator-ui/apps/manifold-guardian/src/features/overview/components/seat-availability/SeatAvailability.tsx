import { SectionCard } from '@operator-ui/common-ui';
import { Link } from 'react-router-dom';
import { describeOffer } from '@/shared/utils/offerPrice';
import styles from './SeatAvailability.module.css';

interface SeatAvailabilityProps {
  priceMsat: number | null;
  /** Whether the network offers guardian codes at all (`badgeless_mg`). */
  guardianCodes: boolean;
  authorized: boolean;
}

// The ways a seat reaches this host. A badge adds Fedi-verified selection
// beside guardian codes; without one, guardian codes are the whole offer and
// there is no price to set (REQ-bring-your-own-guardians). Where the network
// offers no guardian codes, Fedi-verified selection is the only way, badge or not.
export const SeatAvailability = ({
  priceMsat,
  guardianCodes,
  authorized
}: SeatAvailabilityProps) => {
  const badgeless = guardianCodes && !authorized;
  const codes = guardianCodes ? (
    <div className={styles.row}>
      <div className={styles.text}>
        <span className={styles.name}>Guardian codes</span>

        <span className={styles.detail}>
          Federation Ambassadors you share a code with add this guardian directly.
        </span>
      </div>

      <span className={styles.value}>Free setup</span>
    </div>
  ) : null;

  const verified = badgeless ? (
    <div className={styles.row}>
      <div className={styles.text}>
        <span className={styles.name}>Fedi-verified selection</span>

        <span className={styles.detail}>
          Needs a Trusted badge from a Fedi signer. Then Federation Ambassadors can find and select
          this guardian, and you set a seat price.
        </span>
      </div>

      <span className={styles.valueMuted}>Off</span>

      <Link to="/authorization" className={styles.link}>
        Add a badge
      </Link>
    </div>
  ) : (
    <div className={styles.row}>
      <div className={styles.text}>
        <span className={styles.name}>Fedi-verified selection</span>

        <span className={styles.detail}>
          Federation Ambassadors find and select this guardian through your Trusted badge.
        </span>
      </div>

      <span className={styles.value}>{describeOffer(priceMsat)}</span>

      <Link to="/offer" className={styles.link}>
        Change price and seats
      </Link>
    </div>
  );

  return (
    <SectionCard title="Seat availability">
      <div className={styles.root}>
        {badgeless ? codes : verified}
        {badgeless ? verified : codes}
      </div>
    </SectionCard>
  );
};
