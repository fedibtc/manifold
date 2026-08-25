import { useState } from 'react';
import { Outlet } from 'react-router-dom';
import { useSetupGate } from '@/features/setup/hooks/use-setup-gate/useSetupGate';
import { SetupPage } from '@/pages/setup/SetupPage';

/**
 * Sits above `AppShell` and replaces it outright, so setup is a full screen
 * with no sidebar rather than a page inside the app.
 *
 * The wizard has to outlive the condition that opened it: applying a config
 * flips setup-state to `ready` immediately, but the operator still has a
 * "you're live" screen to read. So the gate latches on entry and only the
 * wizard closes it. Reacting to `gated` directly would swap the wizard out for
 * the shell the instant validation passed, taking that screen with it.
 *
 * Setup deliberately owns no route. The operator's location is untouched while
 * the wizard is up and resumes underneath once the gate lifts, which is also
 * why there is no redirect to unwind here.
 */
export const SetupGate = () => {
  const { gated } = useSetupGate();
  const [isSettingUp, setIsSettingUp] = useState(false);

  // Guarded setState during render, not an effect — the compiler forbids
  // setState inside useEffect, and this is the sanctioned "adjust state on
  // data change" shape.
  if (!isSettingUp && gated) {
    setIsSettingUp(true);
  }

  const handleComplete = () => {
    setIsSettingUp(false);
  };

  if (isSettingUp) return <SetupPage onComplete={handleComplete} />;

  return <Outlet />;
};
