import { NAV_ITEMS } from '@/app/components/navigation-items/nav-config';
import { NavigationItem } from '@/app/components/navigation-items/navigation-item/NavigationItem';
import { useSupportChat } from '@/features/support/api/hooks/use-support-chat/useSupportChat';
import { SUPPORT_UNREAD_POLL_MS } from '@/shared/api/pollingIntervals';

export const NavigationItems = () => {
  const unreadSupport = useSupportChat(SUPPORT_UNREAD_POLL_MS).data?.unread ?? 0;
  return NAV_ITEMS.map((item) => (
    <NavigationItem key={item.key} item={item} count={item.key === 'support' ? unreadSupport : 0} />
  ));
};
