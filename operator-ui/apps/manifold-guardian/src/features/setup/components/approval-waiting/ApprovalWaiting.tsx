import type { ReactNode } from 'react';
import styles from './ApprovalWaiting.module.css';

interface ApprovalWaitingProps {
  lastChecked: string;
  action?: ReactNode;
}

export const ApprovalWaiting = ({ lastChecked, action }: ApprovalWaitingProps) => (
  <section className={styles.root} aria-labelledby="approval-waiting-title">
    <svg className={styles.icon} viewBox="0 0 24 24" aria-hidden="true">
      <circle cx="12" cy="12" r="9" />

      <path d="M12 7.5V12l3 2" />
    </svg>

    <div className={styles.body}>
      <h2 id="approval-waiting-title" className={styles.title}>
        Waiting for approval
      </h2>

      <p className={styles.text}>
        Until then it is not advertised and cannot sell seats. Last checked {lastChecked}.
      </p>
    </div>

    {action ? <div className={styles.action}>{action}</div> : null}
  </section>
);
