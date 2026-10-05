import { render, screen } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { beforeEach, vi } from 'vitest';
import { NAV_ITEMS } from '@/app/components/navigation-items/nav-config';
import { useSupportChat } from '@/features/support/api/hooks/use-support-chat/useSupportChat';
import { useOnboarding } from '@/shared/api/hooks/use-onboarding/useOnboarding';
import { AppShell } from '../AppShell';

vi.mock('@/shared/api/hooks/use-onboarding/useOnboarding');
vi.mock('@/features/support/api/hooks/use-support-chat/useSupportChat');

const useOnboardingMock = vi.mocked(useOnboarding);

beforeEach(() => {
  vi.mocked(useSupportChat).mockReturnValue({ data: undefined } as ReturnType<
    typeof useSupportChat
  >);
  useOnboardingMock.mockReturnValue({
    data: {
      fman_name: 'blissful-chiffchaff',
      service_pubkey: 'abc',
      nostr: { state: 'not_observed', checked_at: 1_760_000_000 }
    }
  } as ReturnType<typeof useOnboarding>);
});

const renderShellAt = (path: string) =>
  render(
    <MemoryRouter initialEntries={[path]}>
      <Routes>
        <Route element={<AppShell />}>
          <Route index element={<div>overview content</div>} />

          <Route path="seats" element={<div>seats content</div>} />
        </Route>
      </Routes>
    </MemoryRouter>
  );

it('should render the brand and every nav item', () => {
  renderShellAt('/');

  screen.getByText('Manifold Fedimint Guardian');
  screen.getByText('blissful-chiffchaff');
  for (const item of NAV_ITEMS) {
    screen.getByRole('link', { name: item.label });
  }
});

it('should render the routed page content in the outlet', () => {
  renderShellAt('/seats');

  screen.getByText('seats content');
});
