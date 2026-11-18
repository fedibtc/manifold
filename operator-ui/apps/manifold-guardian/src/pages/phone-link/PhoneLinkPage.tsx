import { useEffect } from 'react';
import { Navigate } from 'react-router-dom';
import { useGuardianLink } from '@/features/phone-link/api/hooks/use-guardian-link/useGuardianLink';
import { GuardianLink } from '@/features/phone-link/components/guardian-link/GuardianLink';
import { MOBILE_NOTIFICATIONS_NEWS_ID } from '@/features/phone-link/hooks/use-mobile-notifications-news/useMobileNotificationsNews';
import styles from '@/pages/phone-link/PhoneLinkPage.module.css';
import { markWhatsNewSeen } from '@/shared/whats-new/whatsNew';

interface PhoneLinkPageProps extends Record<string, never> {}

export const PhoneLinkPage = (_props: PhoneLinkPageProps) => {
  const status = useGuardianLink();
  const ready = status.data?.availability === 'ready';
  // Opening the page is what the Overview announcement and the nav dot ask for.
  useEffect(() => {
    if (ready) markWhatsNewSeen(MOBILE_NOTIFICATIONS_NEWS_ID);
  }, [ready]);
  // Where the policy keeps mobile notifications off, the page does not exist.
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
