import { Outlet } from 'react-router-dom';
import { AccessDenied } from '@/features/boot/components/access-denied/AccessDenied';
import { BootLoading } from '@/features/boot/components/boot-loading/BootLoading';
import { DaemonError } from '@/features/boot/components/daemon-error/DaemonError';
import { DaemonStarting } from '@/features/boot/components/daemon-starting/DaemonStarting';
import { useBootStatus } from '@/features/boot/hooks/use-boot-status/useBootStatus';
import { AuthPromptPage } from '@/pages/auth/AuthPromptPage';
import { RestoreConsolePage } from '@/pages/restore-console/RestoreConsolePage';

// Boot order (see useBootStatus): (1) daemon unreachable → G1; (2) restore
// mode → standalone recovery console (no shell, not a route); (3) unauthorized
// → G2; (4) access denied → permission screen; (5) shell + routes.
export const BootGate = () => {
  const { status, onRetry } = useBootStatus();

  if (status === 'booting') return <BootLoading />;
  if (status === 'needs-auth') return <AuthPromptPage />;
  if (status === 'access-denied') return <AccessDenied onRetry={onRetry} />;
  if (status === 'daemon-unreachable') return <DaemonError onRetry={onRetry} />;
  if (status === 'reloading' || status === 'no-runtime') {
    return <DaemonStarting reason={status} onRetry={onRetry} />;
  }
  if (status === 'restore-mode') return <RestoreConsolePage />;

  return <Outlet />;
};
