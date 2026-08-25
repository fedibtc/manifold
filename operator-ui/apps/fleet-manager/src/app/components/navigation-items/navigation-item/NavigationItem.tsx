import { NavLink } from 'react-router-dom';
import type { NavItem } from '@/app/components/navigation-items/nav-config';
import styles from './NavigationItem.module.css';

interface NavRowProps {
  item: NavItem;
}

export const NavigationItem = ({ item }: NavRowProps) => (
  <NavLink
    to={item.path}
    end={item.path === '/'}
    className={({ isActive }) => (isActive ? styles.active : styles.idle)}
  >
    {item.label}
  </NavLink>
);
