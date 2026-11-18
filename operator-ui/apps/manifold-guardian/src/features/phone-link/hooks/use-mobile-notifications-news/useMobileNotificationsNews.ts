import { useGuardianLink } from '@/features/phone-link/api/hooks/use-guardian-link/useGuardianLink';
import { GUARDIAN_LINK_NAV_POLL_MS } from '@/shared/api/pollingIntervals';
import { useWhatsNewSeen, type WhatsNewItem } from '@/shared/whats-new/whatsNew';

export const MOBILE_NOTIFICATIONS_NEWS_ID = 'mobile-notifications';

const item: WhatsNewItem = {
  id: MOBILE_NOTIFICATIONS_NEWS_ID,
  title: 'Get notified on your phone',
  body: 'Link the Fedi app to hear when your guardian needs attention.',
  action: { label: 'Set up', path: '/mobile-notifications' }
};

/** Shown only where a phone can be linked now, and until one is, the page has
 *  been opened, or the host chose "Not now". */
export const useMobileNotificationsNews = (): WhatsNewItem | null => {
  const phone = useGuardianLink(GUARDIAN_LINK_NAV_POLL_MS).data;
  const seen = useWhatsNewSeen(MOBILE_NOTIFICATIONS_NEWS_ID);
  return !seen && phone?.availability === 'ready' && !phone.link ? item : null;
};
