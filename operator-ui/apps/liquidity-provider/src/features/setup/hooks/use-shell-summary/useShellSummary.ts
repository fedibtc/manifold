import { useSetupState } from '@/shared/api/hooks/use-setup-state/useSetupState';

// Shell display state: nav items unlock once setup is ready; the footer shows
// the configured network when known.
export const useShellSummary = (): { ready: boolean; network: string | undefined } => {
  const setup = useSetupState();

  const ready = setup.data?.status === 'ready';
  const network = setup.data?.config?.network;

  return { ready, network };
};
