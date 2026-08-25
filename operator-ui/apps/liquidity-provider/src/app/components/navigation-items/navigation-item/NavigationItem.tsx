import { NavLink } from 'react-router-dom';
import type { NavItem } from '@/app/components/navigation-items/nav-config';
import styles from './NavigationItem.module.css';

interface NavRowProps {
  item: NavItem;
  disabled: boolean;
}

export const NavigationItem = ({ item, disabled }: NavRowProps) => {
  if (disabled) {
    return (
      <span className={styles.disabled} aria-disabled="true" title="Complete setup first">
        {item.label}
      </span>
    );
  }

  return (
    <NavLink
      to={item.path}
      end={item.path === '/'}
      className={({ isActive }) => (isActive ? styles.active : styles.idle)}
    >
      {item.label}
    </NavLink>
  );
};
