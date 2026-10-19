import type { OnboardingResponse } from '@operator-ui/types';
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { MOCK_SERVICE_NOSTR_PUBKEY } from '@/mocks/world/keys';
import { NetworkError } from '@/shared/api/errors';
import { AuthorizationStateGuard } from '../AuthorizationStateGuard';

const data: OnboardingResponse = {
  stage: 'holder_authorization',
  runtime: 'starting',
  fman_name: 'mutual-hamster',
  service_pubkey: '02abc',
  service_nostr_pubkey: MOCK_SERVICE_NOSTR_PUBKEY,
  nostr: { state: 'not_observed', checked_at: 1_760_000_000 },
  fman_version: { current: '0.1.0', latest: '0.1.0', update_required: false }
};

const renderContent = (state: OnboardingResponse) => <p>{state.service_nostr_pubkey}</p>;

describe('AuthorizationStateGuard', () => {
  it('should show the error alone when no state has been read', () => {
    render(
      <AuthorizationStateGuard data={undefined} isLoading={false} error={new NetworkError()}>
        {renderContent}
      </AuthorizationStateGuard>
    );

    expect(screen.getByText(/can't reach manifold fedimint guardian/i)).toBeTruthy();
    expect(screen.queryByText(/could not be refreshed/i)).toBeNull();
  });

  it('should say the state is being read while the first read is outstanding', () => {
    render(
      <AuthorizationStateGuard data={undefined} isLoading error={null}>
        {renderContent}
      </AuthorizationStateGuard>
    );

    expect(screen.getByText('Reading the authorization state…')).toBeTruthy();
  });

  it('should render nothing when there is no state, no read and no error', () => {
    const { container } = render(
      <AuthorizationStateGuard data={undefined} isLoading={false} error={null}>
        {renderContent}
      </AuthorizationStateGuard>
    );

    expect(container.textContent).toBe('');
  });

  it('should render its content from the state it was given', () => {
    render(
      <AuthorizationStateGuard data={data} isLoading={false} error={null}>
        {renderContent}
      </AuthorizationStateGuard>
    );

    expect(screen.getByText(MOCK_SERVICE_NOSTR_PUBKEY)).toBeTruthy();
    expect(screen.queryByText(/could not be refreshed/i)).toBeNull();
  });

  it('should keep the content and warn when a later read fails', () => {
    render(
      <AuthorizationStateGuard data={data} isLoading={false} error={new NetworkError()}>
        {renderContent}
      </AuthorizationStateGuard>
    );

    expect(screen.getByText(MOCK_SERVICE_NOSTR_PUBKEY)).toBeTruthy();
    expect(screen.getByText(/this state could not be refreshed/i)).toBeTruthy();
  });
});
