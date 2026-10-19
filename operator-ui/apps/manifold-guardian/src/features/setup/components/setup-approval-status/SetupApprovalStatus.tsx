import type { OnboardingNostrStatus } from '@operator-ui/types';
import type { ReactNode } from 'react';
import { ApprovalWaiting } from '@/features/setup/components/approval-waiting/ApprovalWaiting';
import { AuthorizationStatusBanner } from '@/shared/components/authorization-status-banner/AuthorizationStatusBanner';
import { RelayErrorBanner } from '@/shared/components/relay-error-banner/RelayErrorBanner';
import { formatCheckedAt } from '@/shared/utils/format';

interface SetupApprovalStatusProps {
  nostr: OnboardingNostrStatus;
  action: ReactNode;
}

export const SetupApprovalStatus = ({ nostr, action }: SetupApprovalStatusProps) => {
  if (nostr.state === 'not_observed') {
    return <ApprovalWaiting lastChecked={formatCheckedAt(nostr.checked_at)} action={action} />;
  }

  if (nostr.state === 'relay_error') {
    return <RelayErrorBanner error={nostr.error} action={action} />;
  }

  return <AuthorizationStatusBanner nostr={nostr} />;
};
