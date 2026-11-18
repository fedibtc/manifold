import { Banner } from '@operator-ui/common-ui';
import type { OnboardingNostrStatus } from '@operator-ui/types';
import { RelayErrorBanner } from '@/shared/components/relay-error-banner/RelayErrorBanner';
import { formatCheckedAt } from '@/shared/utils/format';

interface AuthorizationStatusBannerProps {
  nostr: OnboardingNostrStatus;
}

// One banner per daemon state, because the four states are four different facts
// and the operator acts differently on each. "Not read yet" means nothing is
// stored and the relays have not been asked since startup — the daemon reads
// them only when the operator asks, so it does not clear on its own, and a host
// set up without a badge stays in it; "read, nothing found" is the state a
// fleet waits in for a holder to sign; a relay error is the operator's to
// chase. Folding any of them together would produce a sentence that is true of
// one and false of another.
export const AuthorizationStatusBanner = ({ nostr }: AuthorizationStatusBannerProps) => {
  // What an unapproved host loses is the operator's first question: the daemon
  // advertises only with a stored approval (SPEC-advertisement).
  if (nostr.state === 'checking') {
    return (
      <Banner variant="info" title="Not approved">
        This host has no stored approval, so it is not advertised and cannot sell seats. Approval
        has not been checked since the app started.
      </Banner>
    );
  }

  if (nostr.state === 'not_observed') {
    return (
      <Banner variant="info" title="Not approved yet">
        Until this host is approved it is not advertised and cannot sell seats. Last checked{' '}
        {formatCheckedAt(nostr.checked_at)}.
      </Banner>
    );
  }

  if (nostr.state === 'relay_error') {
    return <RelayErrorBanner error={nostr.error} />;
  }

  // An observed authorization with no check time came out of the daemon's
  // retained store. Saying so matters: it is still valid — retained
  // authorizations are re-verified before reuse — but it was not confirmed
  // against the relay during this run.
  return (
    <Banner variant="success" title="Approved">
      This host is approved. It is advertised once setup is complete.{' '}
      {nostr.checked_at === null
        ? 'Confirmed from the stored record; not re-checked since startup.'
        : `Confirmed at ${formatCheckedAt(nostr.checked_at)}.`}
    </Banner>
  );
};
