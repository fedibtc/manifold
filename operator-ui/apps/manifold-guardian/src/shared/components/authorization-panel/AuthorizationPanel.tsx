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

        {/* The holder side is credential-app PR #132, which accepts exactly this
              payload: `parseHolderAuthorizationRequest` takes
              `{ subject_pubkey: <64 hex> }` through a `.strict()` schema, and the
              same request serves MG and FLIP. Strict is why
              `buildAuthorizationRequest` may never gain a field — an added `type` or
              `version` would be rejected outright, so BE-FMAN-AUTH-002 has to be
              agreed on both sides at once. */}
        {/* Deliberately not "guardian ID": a guardian is one fedimintd seat and a
              fleet hosts many, so naming the fleet-wide key after a single guardian
              would be wrong. */}
        <p className={styles.hint}>
          This is your Manifold Fedimint Guardian ID. Scan it with the PeerBadge app, or copy the
          request and paste it there.
        </p>

        <AuthorizationStatusBanner nostr={state.nostr} />
      </>
    )}
  </AuthorizationStateGuard>
);
