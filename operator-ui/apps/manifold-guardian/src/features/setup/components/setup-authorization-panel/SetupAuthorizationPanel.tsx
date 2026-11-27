import type { OnboardingResponse } from '@operator-ui/types';
import { SetupApprovalStatus } from '@/features/setup/components/setup-approval-status/SetupApprovalStatus';
import { AuthorizationStateGuard } from '@/shared/components/authorization-state-guard/AuthorizationStateGuard';
import { PeerBadgeApprovalSteps } from '@/shared/components/peerbadge-approval-steps/PeerBadgeApprovalSteps';

interface SetupAuthorizationPanelProps {
  data: OnboardingResponse | undefined;
  isLoading: boolean;
  error: unknown;
}

export const SetupAuthorizationPanel = ({
  data,
  isLoading,
  error
}: SetupAuthorizationPanelProps) => (
  <AuthorizationStateGuard data={data} isLoading={isLoading} error={error}>
    {(state) => (
      <>
        <PeerBadgeApprovalSteps data={state} />

        <SetupApprovalStatus nostr={state.nostr} />
      </>
    )}
  </AuthorizationStateGuard>
);
