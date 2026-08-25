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
    return <Banner variant="info">Reading the relay for the first time since startup.</Banner>;
  }

  if (nostr.state === 'not_observed') {
    return (
      <Banner variant="info">
        No authorization for this fleet was on the relay when it was last read (
        {formatCheckedAt(nostr.checked_at)}).
      </Banner>
    );
  }

  if (nostr.state === 'relay_error') {
    return (
      <Banner variant="warn">
        The relay could not be read, so nothing is known about this fleet's authorization:{' '}
        {nostr.error}
      </Banner>
    );
  }

  // An observed authorization with no check time came out of the daemon's
  // retained store. Saying so matters: it is still valid — retained
  // authorizations are re-verified before reuse — but it was not confirmed
  // against the relay during this run.
  return (
    <Banner variant="success">
      Authorization observed. This fleet can be evaluated.{' '}
      {nostr.checked_at === null
        ? 'Confirmed from the stored record; the relay has not been read since startup.'
        : `Confirmed against the relay at ${formatCheckedAt(nostr.checked_at)}.`}
    </Banner>
  );
};
