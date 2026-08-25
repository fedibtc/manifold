import { useEffect, useState } from 'react';
import { Outlet } from 'react-router-dom';
import { SetupWizard } from '@/features/setup/components/setup-wizard/SetupWizard';
import { isNotOnboardedError } from '@/features/setup/utils/setupState';
import { useOnboarding } from '@/shared/api/hooks/use-onboarding/useOnboarding';
import { gateSurface } from '@/shared/surface/gateSurface';

/**
 * Sits above `AppShell`, so no sidebar renders while a host is being set up.
 *
 * The wizard has to outlive the condition that opened it: the very first step
 * onboards the daemon, after which every verb answers and the "not onboarded"
 * signal is gone — but the operator still has a phrase to write down, a QR to
 * show, and a price to set. So the gate latches once and only the wizard closes
 * it.
 */
export const SetupGate = () => {
  const onboarding = useOnboarding();
  const [isSettingUp, setIsSettingUp] = useState(false);
  const requiresSetup =
    isNotOnboardedError(onboarding.error) ||
    (onboarding.data !== undefined && onboarding.data.runtime !== 'ready');
  // Guarded setState during render, not an effect — the compiler forbids setState
  // inside useEffect, and this is the sanctioned "adjust state on data change" shape.
  if (!isSettingUp && requiresSetup) {
    setIsSettingUp(true);
  }
  if (isSettingUp && onboarding.data?.runtime === 'ready') {
    setIsSettingUp(false);
  }

  // The wizard has no route of its own, so it names itself for the dev mock
  // panel. Only this owner's value is touched, so BootGate's cleanup above
  // cannot clear it.
  useEffect(() => {
    if (!isSettingUp) return;

    gateSurface.set('setup', 'setup');

    return () => gateSurface.clear('setup');
  }, [isSettingUp]);

  const handleComplete = () => {
    void onboarding.refetch();
  };

  const initialStep =
    onboarding.data?.stage === 'initial_offer'
      ? 'price'
      : onboarding.data?.stage === 'holder_authorization'
        ? 'authorization'
        : 'doors';

  if (isSettingUp) {
    return <SetupWizard onComplete={handleComplete} initialStep={initialStep} />;
  }

  return <Outlet />;
};
