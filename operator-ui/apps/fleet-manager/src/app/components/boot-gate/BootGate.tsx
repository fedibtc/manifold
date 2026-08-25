import { useEffect } from 'react';
import { Outlet } from 'react-router-dom';
import { BootLoading } from '@/features/boot/components/boot-loading/BootLoading';
import { DaemonError } from '@/features/boot/components/daemon-error/DaemonError';
import {
  type BootStatus,
  useBootStatus
} from '@/features/boot/hooks/use-boot-status/useBootStatus';
import { AuthPromptPage } from '@/pages/auth/AuthPromptPage';
import { type GateSurface, gateSurface } from '@/shared/surface/gateSurface';

// None of these three screens has a route of its own, so the dev mock panel
// cannot name them from the pathname. `access-denied` shares the daemon-error
// screen, so it names that same surface. `ready` owns no surface: the routed tree
// below is what is on the screen then.
const SURFACE_OF: Record<BootStatus, GateSurface | null> = {
  booting: 'boot',
  'needs-auth': 'auth',
  'daemon-unreachable': 'daemon-error',
  'access-denied': 'daemon-error',
  ready: null
};

// Both mean the daemon is not serving this dashboard, and both leave the operator
// the same Retry, so they share G1. Which of the two it is comes from the failure
// the screen is handed, not from a second screen.
const DAEMON_FAILURE_STATUSES: readonly BootStatus[] = ['daemon-unreachable', 'access-denied'];

// Boot order (see useBootStatus): (1) unauthorized → G2; (2) refused or unreachable
// → G1; (3) shell + routes.
export const BootGate = () => {
  const { status, failure, onRetry } = useBootStatus();

  // An effect, not a render-time write: writing to an external store during
  // render breaks the React Compiler's purity rule. StrictMode's double invoke
  // runs set → cleanup → set, which the owner key makes idempotent.
  useEffect(() => {
    const surface = SURFACE_OF[status];
    if (!surface) {
      gateSurface.clear('boot');
      return;
    }

    gateSurface.set('boot', surface);

    return () => gateSurface.clear('boot');
  }, [status]);

  if (status === 'booting') return <BootLoading />;
  if (status === 'needs-auth') return <AuthPromptPage />;
  if (DAEMON_FAILURE_STATUSES.includes(status))
    return <DaemonError failure={failure} onRetry={onRetry} />;

  return <Outlet />;
};
