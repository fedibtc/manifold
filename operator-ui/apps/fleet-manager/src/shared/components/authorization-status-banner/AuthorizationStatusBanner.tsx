import { Banner } from '@operator-ui/common-ui';
import type { OnboardingNostrStatus } from '@operator-ui/types';
import { formatCheckedAt } from '@/shared/utils/format';

interface AuthorizationStatusBannerProps {
  nostr: OnboardingNostrStatus;
}

// One banner per daemon state, because the four states are four different facts
// and the operator acts differently on each. "Not read yet" is transient and
// needs no action; "read, nothing found" is the state a fleet waits in for a
// holder to sign; a relay error is the operator's to chase. Folding any of them
// together would produce a sentence that is true of one and false of another.
export const AuthorizationStatusBanner = ({ nostr }: AuthorizationStatusBannerProps) => {
  if (nostr.state === 'checking') {
    return <Banner variant="info">Checking whether your fleet has been approved…</Banner>;
  }

  // What an unapproved fleet loses is the operator's first question, and it is
  // not cosmetic: the daemon blocks on onboarding before it opens the seat
  // runtime, the iroh endpoint or the advertiser (crates/fman/bin/src/main.rs).
  if (nostr.state === 'not_observed') {
    return (
      <Banner variant="info" title="Not approved yet">
        Until your fleet is approved it is not advertised and cannot sell seats. Last checked{' '}
        {formatCheckedAt(nostr.checked_at)}.
      </Banner>
    );
  }

  if (nostr.state === 'relay_error') {
    return (
      <Banner variant="warn" title="Approval could not be checked">
        Your fleet may or may not be approved — this is a connection problem, not a refusal:{' '}
        {nostr.error}
      </Banner>
    );
  }

  // An observed authorization with no check time came out of the daemon's
  // retained store. Saying so matters: it is still valid — retained
  // authorizations are re-verified before reuse — but it was not confirmed
  // against the relay during this run.
  return (
    <Banner variant="success" title="Approved">
      Your fleet is now available to others.{' '}
      {nostr.checked_at === null
        ? 'Confirmed from the stored record; not re-checked since startup.'
        : `Confirmed at ${formatCheckedAt(nostr.checked_at)}.`}
    </Banner>
  );
};
