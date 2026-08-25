import { Outlet } from 'react-router-dom';
import { NavigationItems } from '@/app/components/navigation-items/NavigationItems';
import { UpdateRequiredTakeover } from '@/features/update/components/update-required-takeover/UpdateRequiredTakeover';
import { useOnboarding } from '@/shared/api/hooks/use-onboarding/useOnboarding';
import styles from './AppShell.module.css';

export const AppShell = () => {
  const onboarding = useOnboarding();

  return (
    <div className={styles.shell}>
      {/* Mounted here rather than in the gate chain above, so it can never
          interrupt sign-in or setup. The shell survives navigation, so a
          dismissal holds for the rest of the session and only a reload brings
          it back. It decides for itself whether to render anything. */}
      <UpdateRequiredTakeover />

      <nav className={styles.sidebar} aria-label="Sections">
        <div className={styles.brand}>
          FMan
          <div className={styles.brandSubtitle}>
            {onboarding.data?.fman_name ?? 'Fleet manager admin'}
          </div>
        </div>

        <NavigationItems />

        <div className={styles.footer}>fleet-manager</div>
      </nav>

      <main className={styles.main}>
        <Outlet />
      </main>
    </div>
  );
};
