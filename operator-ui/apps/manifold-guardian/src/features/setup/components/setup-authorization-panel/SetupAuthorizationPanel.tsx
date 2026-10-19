import type { OnboardingResponse } from '@operator-ui/types';
import type { ReactNode } from 'react';
import { SetupApprovalStatus } from '@/features/setup/components/setup-approval-status/SetupApprovalStatus';
import { AuthorizationStateGuard } from '@/shared/components/authorization-state-guard/AuthorizationStateGuard';
import { PeerBadgeApprovalSteps } from '@/shared/components/peerbadge-approval-steps/PeerBadgeApprovalSteps';

interface SetupAuthorizationPanelProps {
  data: OnboardingResponse | undefined;
  isLoading: boolean;
  error: unknown;
  statusAction: ReactNode;
}

export const SetupAuthorizationPanel = ({
  data,
  isLoading,
  error,
  statusAction
}: SetupAuthorizationPanelProps) => (
  <AuthorizationStateGuard data={data} isLoading={isLoading} error={error}>
    {(state) => (
      <>
        <PeerBadgeApprovalSteps data={state} />

        <SetupApprovalStatus nostr={state.nostr} action={statusAction} />
      </>
    )}
  </AuthorizationStateGuard>
);
