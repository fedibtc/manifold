import { useSetupState } from '@/shared/api/hooks/use-setup-state/useSetupState';

// Gates the app to /setup until setup reaches `ready`. Reactive to the
// setup-state query, so the gate lifts without a reload once validation passes.
export const useSetupGate = (): { gated: boolean } => {
  const setup = useSetupState();

  const status = setup.data?.status;
  const gated = status === 'not_configured' || status === 'pending_validation';

  return { gated };
};
