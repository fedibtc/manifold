import { GuardianLink } from '@/features/fedi-app/components/guardian-link/GuardianLink';
import styles from '@/pages/fedi-app/FediAppPage.module.css';

interface FediAppPageProps extends Record<string, never> {}

export const FediAppPage = (_props: FediAppPageProps) => (
  <div className={styles.root}>
    <div className={styles.pageHead}>
      <h1 className={styles.heading}>Fedi app</h1>
    </div>

    <GuardianLink />
  </div>
);
