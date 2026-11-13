import { Navigate } from 'react-router-dom';
import { useGuardianLink } from '@/features/phone-link/api/hooks/use-guardian-link/useGuardianLink';
import { GuardianLink } from '@/features/phone-link/components/guardian-link/GuardianLink';
import styles from '@/pages/phone-link/PhoneLinkPage.module.css';

interface PhoneLinkPageProps extends Record<string, never> {}

export const PhoneLinkPage = (_props: PhoneLinkPageProps) => {
  const status = useGuardianLink();
  // Where the policy keeps phone link off, the page does not exist.
  if (status.data?.availability === 'disabled') return <Navigate to="/" replace />;
  return (
    <div className={styles.root}>
      <div className={styles.pageHead}>
        <h1 className={styles.heading}>Stay connected to your guardian</h1>

        <p className={styles.intro}>
          Alert notifications in the Fedi app on your phone, wherever you are.
        </p>
      </div>

      <GuardianLink />
    </div>
  );
};
