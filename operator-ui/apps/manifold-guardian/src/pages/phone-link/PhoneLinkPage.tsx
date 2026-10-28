import { GuardianLink } from '@/features/phone-link/components/guardian-link/GuardianLink';
import styles from '@/pages/phone-link/PhoneLinkPage.module.css';

interface PhoneLinkPageProps extends Record<string, never> {}

export const PhoneLinkPage = (_props: PhoneLinkPageProps) => (
  <div className={styles.root}>
    <div className={styles.pageHead}>
      <h1 className={styles.heading}>Phone link</h1>

      <p className={styles.intro}>
        Get a notification on your phone when your guardians need attention or guardian support
        writes to you.
      </p>
    </div>

    <GuardianLink />
  </div>
);
