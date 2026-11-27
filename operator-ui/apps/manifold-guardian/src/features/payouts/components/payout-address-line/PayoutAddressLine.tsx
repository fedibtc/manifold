import { Button } from '@operator-ui/common-ui';
import styles from './PayoutAddressLine.module.css';

interface PayoutAddressLineProps {
  /** The stored Lightning payout address, or null when none is saved. */
  destination: string | null;
  onChange: () => void;
}

/** The one Lightning payout address, on one line; editing opens its dialog. */
export const PayoutAddressLine = ({ destination, onChange }: PayoutAddressLineProps) => (
  <div className={styles.root}>
    {destination === null ? (
      <>
        <span className={styles.empty}>
          No payout address yet. Add one to withdraw over Lightning. Ecash works without it.
        </span>

        <Button size="small" variant="secondary" onClick={onChange}>
          Add payout address
        </Button>
      </>
    ) : (
      <>
        <span className={styles.label}>Lightning payout address</span>

        <span className={styles.address}>{destination}</span>

        <button type="button" className={styles.link} onClick={onChange}>
          Change
        </button>
      </>
    )}
  </div>
);
