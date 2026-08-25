import { NAV_ITEMS } from '@/app/components/navigation-items/nav-config';
import { NavigationItem } from '@/app/components/navigation-items/navigation-item/NavigationItem';
import { useShellSummary } from '@/features/setup/hooks/use-shell-summary/useShellSummary';

export const NavigationItems = () => {
  const { ready } = useShellSummary();

  // No escape hatch for a setup row any more — the shell does not carry one.
  // The disable still matters: the shell can be mounted while setup-state is
  // between answers, and a row leading to a page the daemon cannot serve yet
  // is worse than a disabled one.
  return NAV_ITEMS.map((item) => <NavigationItem key={item.key} item={item} disabled={!ready} />);
};
