import type { ReactNode } from 'react';
import styles from './PeerBadgeStepHead.module.css';

interface PeerBadgeStepHeadProps {
  number: number;
  title: string;
  children: ReactNode;
}

export const PeerBadgeStepHead = ({ number, title, children }: PeerBadgeStepHeadProps) => (
  <div className={styles.root}>
    <span className={styles.number}>{number}</span>

    <div className={styles.copy}>
      <h3 className={styles.title}>{title}</h3>

      <p className={styles.detail}>{children}</p>
    </div>
  </div>
);
