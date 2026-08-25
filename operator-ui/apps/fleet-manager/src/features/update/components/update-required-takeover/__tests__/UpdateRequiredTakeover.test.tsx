import type { FmanVersionReport, OnboardingResponse } from '@operator-ui/types';
import { fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { useOnboarding } from '@/shared/api/hooks/use-onboarding/useOnboarding';
import { UpdateRequiredTakeover } from '../UpdateRequiredTakeover';

vi.mock('@/shared/api/hooks/use-onboarding/useOnboarding', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/shared/api/hooks/use-onboarding/useOnboarding')>()),
  useOnboarding: vi.fn()
}));

const useOnboardingMock = vi.mocked(useOnboarding);

const arrangeVersion = (version: FmanVersionReport | undefined) => {
  const data =
    version === undefined ? undefined : ({ fman_version: version } as OnboardingResponse);
  useOnboardingMock.mockReturnValue({ data } as ReturnType<typeof useOnboarding>);
};

const heading = () => screen.queryByRole('heading', { name: 'Update this Fleet Manager' });

beforeEach(() => {
  vi.clearAllMocks();
});

describe('UpdateRequiredTakeover', () => {
  it('should name both versions when the daemon reports an update is required', () => {
    arrangeVersion({ current: '0.1.0', latest: '0.2.0', update_required: true });

    render(<UpdateRequiredTakeover />);

    expect(heading()).toBeTruthy();
    expect(screen.getByText('0.1.0')).toBeTruthy();
    expect(screen.getByText('0.2.0')).toBeTruthy();
  });

  it('should render nothing when no update is required', () => {
    arrangeVersion({ current: '0.2.0', latest: '0.2.0', update_required: false });

    render(<UpdateRequiredTakeover />);

    expect(heading()).toBeNull();
  });

  it('should render nothing before the onboarding read answers', () => {
    arrangeVersion(undefined);

    render(<UpdateRequiredTakeover />);

    expect(heading()).toBeNull();
  });

  // The daemon should never send this pair. If it does, an empty version on a
  // screen that covers everything is worse than no screen at all.
  it('should render nothing when an update is required but no latest version is named', () => {
    arrangeVersion({ current: '0.1.0', latest: null, update_required: true });

    render(<UpdateRequiredTakeover />);

    expect(heading()).toBeNull();
  });

  it('should return the operator to the dashboard when dismissed', () => {
    arrangeVersion({ current: '0.1.0', latest: '0.2.0', update_required: true });

    render(<UpdateRequiredTakeover />);
    fireEvent.click(screen.getByRole('button', { name: 'Continue to the dashboard' }));

    expect(heading()).toBeNull();
  });

  it('should close on Escape, as any surface covering the screen must', () => {
    arrangeVersion({ current: '0.1.0', latest: '0.2.0', update_required: true });

    render(<UpdateRequiredTakeover />);
    fireEvent.keyDown(window, { key: 'Escape' });

    expect(heading()).toBeNull();
  });

  // Advisory, not a lockout: the operator sees it again next session, and the
  // daemon keeps saying an update is required in between.
  it('should stay dismissed while the onboarding read keeps reporting the update', () => {
    arrangeVersion({ current: '0.1.0', latest: '0.2.0', update_required: true });

    const view = render(<UpdateRequiredTakeover />);
    fireEvent.click(screen.getByRole('button', { name: 'Continue to the dashboard' }));
    view.rerender(<UpdateRequiredTakeover />);

    expect(heading()).toBeNull();
  });

  it('should move focus to the dismiss action so a keyboard reaches it first', () => {
    arrangeVersion({ current: '0.1.0', latest: '0.2.0', update_required: true });

    render(<UpdateRequiredTakeover />);

    expect(document.activeElement).toBe(
      screen.getByRole('button', { name: 'Continue to the dashboard' })
    );
  });

  it('should mark itself as a modal surface for assistive technology', () => {
    arrangeVersion({ current: '0.1.0', latest: '0.2.0', update_required: true });

    render(<UpdateRequiredTakeover />);
    const dialog = screen.getByRole('dialog');

    expect(dialog.getAttribute('aria-modal')).toBe('true');
    expect(dialog.getAttribute('aria-labelledby')).toBe(heading()?.getAttribute('id'));
  });
});
