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
  return NAV_ITEMS.map((item) => (
    <NavigationItem
      key={item.key}
      item={item}
      count={item.key === 'support' ? unreadSupport : 0}
      attention={item.key === 'phone-link' && phoneNeedsAttention}
    />
  ));
};
