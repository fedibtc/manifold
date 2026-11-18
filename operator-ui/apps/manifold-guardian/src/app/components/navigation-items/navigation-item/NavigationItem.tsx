import { NavLink } from 'react-router-dom';
import type { NavItem } from '@/app/components/navigation-items/nav-config';
import styles from './NavigationItem.module.css';

interface NavRowProps {
  item: NavItem;
  /** Things waiting for the operator on that page; zero shows nothing. The
   *  rail shows a dot, as the Fedi app's tab bar does; the count is for screen
   *  readers. */
  count?: number;
  /** Something the operator should do on that page, with nothing to count. */
  attention?: boolean;
  /** A page the operator has not looked at yet. */
  isNew?: boolean;
}

const ariaLabel = (label: string, count: number, attention: boolean, isNew: boolean) => {
  if (count > 0) return `${label}, ${count} unread`;
  if (attention) return `${label}, needs attention`;
  return isNew ? `${label}, new` : undefined;
};

export const NavigationItem = ({
  item,
  count = 0,
  attention = false,
  isNew = false
}: NavRowProps) => (
  <NavLink
    to={item.path}
    end={item.path === '/'}
    aria-label={ariaLabel(item.label, count, attention, isNew)}
    className={({ isActive }) => (isActive ? styles.active : styles.idle)}
  >
    <span className={styles.label}>
      {item.label}
      {count > 0 && <span className={styles.dot} aria-hidden="true" />}
      {count === 0 && attention && <span className={styles.attentionDot} aria-hidden="true" />}
      {count === 0 && !attention && isNew && <span className={styles.newDot} aria-hidden="true" />}
    </span>
  </NavLink>
);
