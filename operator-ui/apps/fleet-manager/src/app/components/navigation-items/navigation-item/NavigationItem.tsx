import { NavLink } from 'react-router-dom';
import type { NavItem } from '@/app/components/navigation-items/nav-config';
import styles from './NavigationItem.module.css';

interface NavRowProps {
  item: NavItem;
  /** Things waiting for the operator on that page; zero shows nothing. The
   *  rail shows a dot, as the Fedi app's tab bar does; the count is for screen
   *  readers. */
  count?: number;
}

export const NavigationItem = ({ item, count = 0 }: NavRowProps) => (
  <NavLink
    to={item.path}
    end={item.path === '/'}
    aria-label={count > 0 ? `${item.label}, ${count} unread` : undefined}
    className={({ isActive }) => (isActive ? styles.active : styles.idle)}
  >
    <span className={styles.label}>
      {item.label}
      {count > 0 && <span className={styles.dot} aria-hidden="true" />}
    </span>
  </NavLink>
);
