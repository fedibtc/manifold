import styles from './GuardianFeeZeroNote.module.css';

// No rate, no minimum and no waiting period are named here. The rate is per
// federation and the remittance minimum is the payer module's, so any figure
// printed in this dashboard would be a guess that goes stale
// (specs/REQ-guardian-fee-remittance.md).
const ZERO_EXPLANATION =
  "Members pay a small fee when they send. Receiving and deposits are free. Fees build up in members' apps first. They're paid out together once every recipient's share is large enough, which can take a few days. Nothing is lost while it waits.";

/**
 * Why every pool can read zero on a fleet that is working correctly. Accrual
 * happens in the payer's app and remits in batches, so a zero here is a wait
 * rather than a fault, and an operator who cannot tell those apart reports an
 * outage that is not one. Collapsed by default: it answers a question the
 * numbers raise, and only for the operator who asks it.
 */
export const GuardianFeeZeroNote = () => (
  <details className={styles.root}>
    <summary className={styles.summary}>Why is this 0?</summary>

    <p className={styles.body}>{ZERO_EXPLANATION}</p>
  </details>
);
