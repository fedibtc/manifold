import type { OnboardingResponse } from '@operator-ui/types';
import { AuthorizationStateGuard } from '@/shared/components/authorization-state-guard/AuthorizationStateGuard';
import { AuthorizationStatusBanner } from '@/shared/components/authorization-status-banner/AuthorizationStatusBanner';
import { PanelAuthorizationCode } from '@/shared/components/panel-authorization-code/PanelAuthorizationCode';
import styles from './AuthorizationPanel.module.css';

interface AuthorizationPanelProps {
  data: OnboardingResponse | undefined;
  isLoading: boolean;
  error: unknown;
}

// Renders state, never actions. The setup step and the standalone page need the
// same key, QR and status but entirely different controls, so the buttons stay
// with each surface.
export const AuthorizationPanel = ({ data, isLoading, error }: AuthorizationPanelProps) => (
  <AuthorizationStateGuard data={data} isLoading={isLoading} error={error}>
    {(state) => (
      <>
        <PanelAuthorizationCode data={state} />

        <p className={styles.hint}>
          This is your Manifold Fedimint Guardian ID. Scan it with the PeerBadge app, or copy the
          request and paste it there.
        </p>

        <AuthorizationStatusBanner nostr={state.nostr} />
      </>
    )}
  </AuthorizationStateGuard>
);
