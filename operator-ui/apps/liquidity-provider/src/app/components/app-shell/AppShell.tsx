import { Outlet } from 'react-router-dom';
import { NavigationItems } from '@/app/components/navigation-items/NavigationItems';
import { useShellSummary } from '@/features/setup/hooks/use-shell-summary/useShellSummary';
import styles from './AppShell.module.css';

export const AppShell = () => {
  const { network } = useShellSummary();

  const footerText = network ? `flipd · ${network}` : 'flipd';

  return (
    <div className={styles.shell}>
      <nav className={styles.sidebar} aria-label="Sections">
        <div className={styles.brand}>
          FLIP
          <div className={styles.brandSubtitle}>Liquidity provider</div>
        </div>

        <NavigationItems />

        <div className={styles.footer}>{footerText}</div>
      </nav>

      <main className={styles.main}>
        <Outlet />
      </main>
    </div>
  );
};
