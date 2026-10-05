import { NavLink } from 'react-router-dom';
import type { NavItem } from '@/app/components/navigation-items/nav-config';
import styles from './NavigationItem.module.css';

interface NavRowProps {
  item: NavItem;
  /** Things waiting for the operator on that page; zero shows nothing. */
  count?: number;
}

export const NavigationItem = ({ item, count = 0 }: NavRowProps) => (
  <NavLink
    to={item.path}
    end={item.path === '/'}
    aria-label={count > 0 ? `${item.label}, ${count} unread` : undefined}
    className={({ isActive }) => (isActive ? styles.active : styles.idle)}
  >
    {item.label}
    {count > 0 && (
      <span className={styles.count} aria-hidden="true">
        {count}
      </span>
    )}
  </NavLink>
);
