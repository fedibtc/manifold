import { NAV_ITEMS } from '@/app/components/navigation-items/nav-config';
import { NavigationItem } from '@/app/components/navigation-items/navigation-item/NavigationItem';
import { useSupportChat } from '@/features/support/api/hooks/use-support-chat/useSupportChat';

export const NavigationItems = () => {
  const unreadSupport = useSupportChat().data?.unread ?? 0;
  return NAV_ITEMS.map((item) => (
    <NavigationItem key={item.key} item={item} count={item.key === 'support' ? unreadSupport : 0} />
  ));
};
