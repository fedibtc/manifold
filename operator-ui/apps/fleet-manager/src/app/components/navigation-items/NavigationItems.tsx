import { NAV_ITEMS } from '@/app/components/navigation-items/nav-config';
import { NavigationItem } from '@/app/components/navigation-items/navigation-item/NavigationItem';

export const NavigationItems = () =>
  NAV_ITEMS.map((item) => <NavigationItem key={item.key} item={item} />);
