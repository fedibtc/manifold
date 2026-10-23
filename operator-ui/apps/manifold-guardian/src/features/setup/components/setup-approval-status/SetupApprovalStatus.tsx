import { Banner } from '@operator-ui/common-ui';
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

  // The daemon reads the relays only when asked, and each run starts with no
  // read. Without the action here, setup has no way out of this state.
  if (nostr.state === 'checking') {
    return (
      <Banner variant="info" title="Not checked yet" action={action}>
        Approval has not been checked since the app started.
      </Banner>
    );
  }

  if (nostr.state === 'relay_error') {
    return <RelayErrorBanner error={nostr.error} action={action} />;
  }

  return <AuthorizationStatusBanner nostr={nostr} />;
};
