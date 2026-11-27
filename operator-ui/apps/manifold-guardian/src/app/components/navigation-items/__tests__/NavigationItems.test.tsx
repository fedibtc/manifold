import { render, screen } from '@testing-library/react';
import { createMemoryRouter, RouterProvider } from 'react-router-dom';
import { beforeEach, vi } from 'vitest';
import { useGuardianLink } from '@/features/phone-link/api/hooks/use-guardian-link/useGuardianLink';
import { useSupportChat } from '@/features/support/api/hooks/use-support-chat/useSupportChat';
import { useOnboarding } from '@/shared/api/hooks/use-onboarding/useOnboarding';
import { GUARDIAN_LINK_NAV_POLL_MS, SUPPORT_UNREAD_POLL_MS } from '@/shared/api/pollingIntervals';
import { NavigationItems } from '../NavigationItems';
import { NAV_ITEMS } from '../nav-config';

vi.mock('@/features/support/api/hooks/use-support-chat/useSupportChat');
vi.mock('@/features/phone-link/api/hooks/use-guardian-link/useGuardianLink');
vi.mock('@/shared/api/hooks/use-onboarding/useOnboarding');

const guardianCodesOffered = (badgelessMg: boolean | undefined) =>
  vi.mocked(useOnboarding).mockReturnValue({
    data: badgelessMg === undefined ? undefined : { badgeless_mg: badgelessMg }
  } as ReturnType<typeof useOnboarding>);

const phoneStatus = (data: unknown) =>
  vi.mocked(useGuardianLink).mockReturnValue({ data } as ReturnType<typeof useGuardianLink>);
const activePhone = { device_label: 'P', delivery: { state: 'active', reason: null } };

const unread = (count: number | undefined) =>
  vi.mocked(useSupportChat).mockReturnValue({
    data: count === undefined ? undefined : { available: true, messages: [], unread: count }
  } as ReturnType<typeof useSupportChat>);

const renderItems = () => {
  const router = createMemoryRouter([{ path: '*', element: <NavigationItems /> }]);
  render(<RouterProvider router={router} />);
};

beforeEach(() => {
  unread(undefined);
  phoneStatus({ availability: 'ready', link: activePhone, offer: null });
  guardianCodesOffered(true);
});

it('should render a link for every configured nav item', () => {
  renderItems();

  for (const item of NAV_ITEMS) {
    screen.getByRole('link', { name: item.label });
  }
});

it('should count unread Fedi messages on Support only', () => {
  unread(2);
  renderItems();

  expect(screen.getByRole('link', { name: 'Support, 2 unread' })).toBeInTheDocument();
  expect(document.querySelectorAll('a [aria-hidden="true"]')).toHaveLength(1);
  // The dot polls at the slow cadence; only the open Support page polls fast.
  expect(useSupportChat).toHaveBeenCalledWith(SUPPORT_UNREAD_POLL_MS);
});

it('should show no dot once everything is read', () => {
  unread(0);
  renderItems();

  expect(screen.getByRole('link', { name: 'Support' })).toBeInTheDocument();
  expect(document.querySelector('a [aria-hidden="true"]')).toBeNull();
});

it('should flag Mobile notifications when notifications stopped', () => {
  phoneStatus({
    availability: 'ready',
    link: { ...activePhone, delivery: { state: 'terminal', reason: 'hook_not_found' } },
    offer: null
  });
  renderItems();

  expect(
    screen.getByRole('link', { name: 'Mobile notifications, needs attention' })
  ).toBeInTheDocument();
  expect(document.querySelectorAll('a [aria-hidden="true"]')).toHaveLength(1);
  expect(useGuardianLink).toHaveBeenCalledWith(GUARDIAN_LINK_NAV_POLL_MS);
});

it('should flag Mobile notifications while the phone is not reached', () => {
  phoneStatus({
    availability: 'ready',
    link: { ...activePhone, delivery: { state: 'inactive', reason: null } },
    offer: null
  });
  renderItems();

  expect(
    screen.getByRole('link', { name: 'Mobile notifications, needs attention' })
  ).toBeInTheDocument();
});

it('should mark Mobile notifications new while no phone is linked', () => {
  phoneStatus({ availability: 'ready', link: null, offer: null });
  renderItems();

  expect(screen.getByRole('link', { name: 'Mobile notifications, new' })).toBeInTheDocument();
});

it.each([
  ['a phone is active', { availability: 'ready', link: activePhone, offer: null }],
  ['this host cannot notify at all', { availability: 'no_push_gateway', link: null, offer: null }]
])('should not flag Mobile notifications when %s', (_name, data) => {
  phoneStatus(data);
  renderItems();

  expect(screen.getByRole('link', { name: 'Mobile notifications' })).toBeInTheDocument();
  expect(document.querySelector('a [aria-hidden="true"]')).toBeNull();
});

it.each([
  ['the policy keeps it off', { availability: 'disabled', link: null, offer: null }],
  ['the status has not loaded', undefined]
])('should hide Mobile notifications when %s', (_name, data) => {
  phoneStatus(data);
  renderItems();

  expect(screen.queryByRole('link', { name: /Mobile notifications/ })).toBeNull();
  expect(screen.getByRole('link', { name: 'Support' })).toBeInTheDocument();
});

it('should show Guardian codes where the network offers badgeless setup', () => {
  renderItems();

  expect(screen.getByRole('link', { name: 'Guardian codes' }).getAttribute('href')).toBe(
    '/guardian-codes'
  );
});

// Production keeps guardian codes off until the whole path ships, like the
// Overview's Seat availability card does.
it.each([
  ['the network does not offer them', false],
  ['onboarding has not loaded', undefined]
])('should hide Guardian codes when %s', (_name, badgelessMg) => {
  guardianCodesOffered(badgelessMg);
  renderItems();

  expect(screen.queryByRole('link', { name: 'Guardian codes' })).toBeNull();
  expect(screen.getByRole('link', { name: 'Seats' })).toBeInTheDocument();
});
