import styles from './GuardianFeeZeroNote.module.css';

const ZERO_EXPLANATION =
  "Members pay a small fee when they send. Receiving and deposits are free. Fees build up in members' apps first. They're paid out together once every recipient's share is large enough, which can take a few days. Nothing is lost while it waits.";

export const GuardianFeeZeroNote = () => (
  <details className={styles.root}>
    <summary className={styles.summary}>Why is this 0?</summary>

    <p className={styles.body}>{ZERO_EXPLANATION}</p>
  </details>
);
