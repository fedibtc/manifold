import { NAV_ITEMS } from '@/app/components/navigation-items/nav-config';
import { NavigationItem } from '@/app/components/navigation-items/navigation-item/NavigationItem';
import { useGuardianLink } from '@/features/phone-link/api/hooks/use-guardian-link/useGuardianLink';
import { useSupportChat } from '@/features/support/api/hooks/use-support-chat/useSupportChat';
import { GUARDIAN_LINK_NAV_POLL_MS, SUPPORT_UNREAD_POLL_MS } from '@/shared/api/pollingIntervals';

export const NavigationItems = () => {
  const unreadSupport = useSupportChat(SUPPORT_UNREAD_POLL_MS).data?.unread ?? 0;
  const phone = useGuardianLink(GUARDIAN_LINK_NAV_POLL_MS).data;
  const phoneState = phone?.link?.delivery.state;
  const phoneNeedsAttention = phoneState === 'inactive' || phoneState === 'terminal';
  // Mobile notifications stay out of sight until the MG says the policy turns it on.
  const phoneLinkShown = phone !== undefined && phone.availability !== 'disabled';
  return NAV_ITEMS.filter((item) => item.key !== 'mobile-notifications' || phoneLinkShown).map(
    (item) => (
      <NavigationItem
        key={item.key}
        item={item}
        count={item.key === 'support' ? unreadSupport : 0}
        attention={item.key === 'mobile-notifications' && phoneNeedsAttention}
      />
    )
  );
};
