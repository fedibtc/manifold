import type { ReactNode } from 'react';
import styles from './TrustStep.module.css';

interface TrustStepProps {
  children: ReactNode;
}

export const TrustStep = ({ children }: TrustStepProps) => (
  <div className={styles.layout}>
    <h2 className={styles.title}>Trust</h2>

    {children}
  </div>
);
