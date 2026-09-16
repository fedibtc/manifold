import { Banner, Button } from '@operator-ui/common-ui';
import { useId } from 'react';
import { GuardianTerms } from '@/shared/components/guardian-terms/GuardianTerms';
import styles from './SetupTerms.module.css';

interface SetupTermsProps {
  onAccepted: () => void;
}

// The daemon stores no record of this acceptance. SetupGate therefore opens a
// reloaded wizard here rather than on the price step, so a reload asks again
// instead of assuming consent.
export const SetupTerms = ({ onAccepted }: SetupTermsProps) => {
  const consentId = useId();

  return (
    <div className={styles.root}>
      <div className={styles.head}>
        <h1 className={styles.heading}>Accept the terms of service</h1>

        <p className={styles.intro}>
          These terms are between you and Fedi. They cover Fedi verification and the telemetry your
          fleet shares with Fedi. Read them before your fleet goes live. Updates are posted at the
          same address.
        </p>
      </div>

      <div className={styles.document}>
        <GuardianTerms />
      </div>

      <Banner variant="info" title="Before you accept">
        The terms include mandatory individual arbitration, and waive class actions and jury trials.
      </Banner>

      <div className={styles.actions}>
        <Button describedBy={consentId} onClick={onAccepted}>
          Accept and continue
        </Button>

        <p id={consentId} className={styles.consent}>
          By selecting Accept and continue, you agree to the Fedi-Verified Guardian Terms of
          Service.
        </p>
      </div>
    </div>
  );
};
