import type { OnboardingResponse } from '@operator-ui/types';

/**
 * The one place the QR's content is decided.
 *
 * This is the PeerBadge SDK's `HolderAuthorizationRequest`: the holder app
 * parses the scanned or pasted text as JSON and accepts `subject_pubkey`
 * alone, so a bare key is rejected before it reaches the badge picker. The
 * same shape is what `manifold-test-issuer` takes on the command line.
 *
 * Lives in `shared` rather than `features/setup` because `pages/authorization`
 * needs it too, and a page may not reach into a feature's utils for it.
 */
export const buildAuthorizationRequest = (onboarding: OnboardingResponse): string =>
  JSON.stringify({ subject_pubkey: onboarding.service_nostr_pubkey });

export const isAuthorized = (onboarding: OnboardingResponse | undefined): boolean =>
  onboarding?.nostr.state === 'authorization_observed';

/**
 * Whether this host runs without a badge. An MG is badgeless exactly while it
 * retains no authorization (SPEC-admin-socket), and only where the network
 * offers badgeless setup; elsewhere a missing authorization is a problem to fix.
 */
export const isBadgeless = (onboarding: OnboardingResponse | undefined): boolean =>
  onboarding?.badgeless_mg === true && !isAuthorized(onboarding);
