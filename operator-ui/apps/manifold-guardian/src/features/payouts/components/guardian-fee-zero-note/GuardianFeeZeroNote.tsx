import styles from './GuardianFeeZeroNote.module.css';

const ZERO_EXPLANATION =
  "Members pay a small fee when they send. Receiving and deposits are free. Fees build up in members' apps first. An app pays your share when that share reaches the federation's minimum deposit. The time this takes depends on how much members send. Nothing is lost while it waits.";

export const GuardianFeeZeroNote = () => (
  <details className={styles.root}>
    <summary className={styles.summary}>Why is this 0?</summary>

    <p className={styles.body}>{ZERO_EXPLANATION}</p>
  </details>
);
