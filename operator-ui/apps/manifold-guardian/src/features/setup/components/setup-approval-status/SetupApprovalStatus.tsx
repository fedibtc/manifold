import { Banner } from '@operator-ui/common-ui';
import type { OnboardingNostrStatus } from '@operator-ui/types';
import { ApprovalWaiting } from '@/features/setup/components/approval-waiting/ApprovalWaiting';
import { AuthorizationStatusBanner } from '@/shared/components/authorization-status-banner/AuthorizationStatusBanner';
import { RelayErrorBanner } from '@/shared/components/relay-error-banner/RelayErrorBanner';
import { formatCheckedAt } from '@/shared/utils/format';

interface SetupApprovalStatusProps {
  nostr: OnboardingNostrStatus;
}

export const SetupApprovalStatus = ({ nostr }: SetupApprovalStatusProps) => {
  if (nostr.state === 'not_observed') {
    return <ApprovalWaiting lastChecked={formatCheckedAt(nostr.checked_at)} />;
  }

  // The daemon reads the relays only when asked, and each run starts with no
  // read. The step's Check approval button is the way out of this state.
  if (nostr.state === 'checking') {
    return (
      <Banner variant="info" title="Not checked yet">
        Approval has not been checked since the app started.
      </Banner>
    );
  }

  if (nostr.state === 'relay_error') {
    return <RelayErrorBanner error={nostr.error} />;
  }

  return <AuthorizationStatusBanner nostr={nostr} />;
};
