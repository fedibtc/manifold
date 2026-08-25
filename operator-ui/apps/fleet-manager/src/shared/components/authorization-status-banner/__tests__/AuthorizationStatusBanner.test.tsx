import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { AuthorizationStatusBanner } from '@/shared/components/authorization-status-banner/AuthorizationStatusBanner';

const CHECKED_AT = 1_760_000_000;

describe('AuthorizationStatusBanner', () => {
  it('should say the relay has not been read yet while the first read is outstanding', () => {
    render(<AuthorizationStatusBanner nostr={{ state: 'checking' }} />);

    expect(screen.getByText(/reading the relay for the first time/i)).toBeTruthy();
  });

  // The daemon separates a completed read from no read. Saying "not observed"
  // for both would describe a fleet nobody has authorized and a fleet nobody has
  // asked about in the same words.
  it('should report a completed read that found nothing, with when it ran', () => {
    render(<AuthorizationStatusBanner nostr={{ state: 'not_observed', checked_at: CHECKED_AT }} />);

    expect(screen.getByText(/no authorization for this fleet/i)).toBeTruthy();
    expect(screen.getByText(/2025-10-09/)).toBeTruthy();
  });

  it('should confirm an observed authorization against the read that found it', () => {
    render(
      <AuthorizationStatusBanner
        nostr={{
          state: 'authorization_observed',
          authorizations: 1,
          holders: ['a'.repeat(64)],
          checked_at: CHECKED_AT
        }}
      />
    );

    expect(screen.getByText(/authorization observed/i)).toBeTruthy();
    expect(screen.getByText(/confirmed against the relay/i)).toBeTruthy();
  });

  // Retained authorizations are durable and re-verified before reuse, so this is
  // still an authorized fleet — but it was not confirmed against the relay during
  // this run, and the operator is told which of the two they are looking at.
  it('should say an observed authorization came from the stored record when no read has succeeded', () => {
    render(
      <AuthorizationStatusBanner
        nostr={{
          state: 'authorization_observed',
          authorizations: 1,
          holders: ['a'.repeat(64)],
          checked_at: null
        }}
      />
    );

    expect(screen.getByText(/from the stored record/i)).toBeTruthy();
  });

  // A relay outage is not evidence that no holder signed. The banner must not
  // read as an absent authorization.
  it('should report a failed read as a failed read', () => {
    render(
      <AuthorizationStatusBanner nostr={{ state: 'relay_error', error: 'connection refused' }} />
    );

    expect(screen.getByText(/relay could not be read/i)).toBeTruthy();
    expect(screen.getByText(/connection refused/i)).toBeTruthy();
    expect(screen.queryByText(/no authorization for this fleet/i)).toBeNull();
  });
});
