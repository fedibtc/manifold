import { Link } from 'react-router-dom';
import styles from './QuickActions.module.css';

interface QuickAction {
  key: string;
  label: string;
  path: string;
}

// One entry per route the app actually serves. "Requests" is deliberately
// absent: the requests workflow is not built (plan task B8, gated on D5), and a
// quick action that routes nowhere is worse than a missing one. Restore it in
// the same change that adds the route.
const QUICK_ACTIONS: QuickAction[] = [
  { key: 'funds', label: 'Funds', path: '/funds' },
  { key: 'advertisement', label: 'Advertisement', path: '/advertisement' },
  { key: 'allocations', label: 'Allocations', path: '/allocations' }
];

const renderAction = (action: QuickAction) => (
  <Link key={action.key} to={action.path} className={styles.action}>
    {action.label}
  </Link>
);

export const QuickActions = () => (
  <section className={styles.root}>
    <h2 className={styles.title}>Quick actions</h2>

    <nav className={styles.nav}>{QUICK_ACTIONS.map(renderAction)}</nav>
  </section>
);
