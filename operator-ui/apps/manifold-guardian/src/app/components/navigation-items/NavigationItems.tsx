import { NAV_ITEMS, type NavItem } from '@/app/components/navigation-items/nav-config';
import { NavigationItem } from '@/app/components/navigation-items/navigation-item/NavigationItem';
import { useGuardianLink } from '@/features/phone-link/api/hooks/use-guardian-link/useGuardianLink';
import { useMobileNotificationsNews } from '@/features/phone-link/hooks/use-mobile-notifications-news/useMobileNotificationsNews';
import { useSupportChat } from '@/features/support/api/hooks/use-support-chat/useSupportChat';
import { useOnboarding } from '@/shared/api/hooks/use-onboarding/useOnboarding';
import { GUARDIAN_LINK_NAV_POLL_MS, SUPPORT_UNREAD_POLL_MS } from '@/shared/api/pollingIntervals';
import { guardianCodesAvailable } from '@/shared/utils/authorization';

export const NavigationItems = () => {
  const unreadSupport = useSupportChat(SUPPORT_UNREAD_POLL_MS).data?.unread ?? 0;
  const phone = useGuardianLink(GUARDIAN_LINK_NAV_POLL_MS).data;
  const phoneState = phone?.link?.delivery.state;
  const phoneNeedsAttention = phoneState === 'inactive' || phoneState === 'terminal';
  // Mobile notifications stay out of sight until the MG says the policy turns it on.
  const phoneLinkShown = phone !== undefined && phone.availability !== 'disabled';
  // Marked new for as long as the Overview announces it.
  const phoneLinkNew = useMobileNotificationsNews() !== null;
  // Guardian codes ship with badgeless setup: where the network does not offer
  // it, the Overview does not offer codes either.
  const guardianCodesShown = guardianCodesAvailable(useOnboarding().data);
  const shown = (item: NavItem) => {
    if (item.key === 'mobile-notifications') return phoneLinkShown;
    if (item.key === 'guardian-codes') return guardianCodesShown;
    return true;
  };
  return NAV_ITEMS.filter(shown).map((item) => (
    <NavigationItem
      key={item.key}
      item={item}
      count={item.key === 'support' ? unreadSupport : 0}
      attention={item.key === 'mobile-notifications' && phoneNeedsAttention}
      isNew={item.key === 'mobile-notifications' && phoneLinkNew}
    />
  ));
};
