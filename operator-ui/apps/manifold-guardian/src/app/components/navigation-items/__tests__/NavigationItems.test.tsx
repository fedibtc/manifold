import { render, screen } from '@testing-library/react';
import { createMemoryRouter, RouterProvider } from 'react-router-dom';
import { beforeEach, vi } from 'vitest';
import { useGuardianLink } from '@/features/phone-link/api/hooks/use-guardian-link/useGuardianLink';
import { useSupportChat } from '@/features/support/api/hooks/use-support-chat/useSupportChat';
import { GUARDIAN_LINK_NAV_POLL_MS, SUPPORT_UNREAD_POLL_MS } from '@/shared/api/pollingIntervals';
import { NavigationItems } from '../NavigationItems';
import { NAV_ITEMS } from '../nav-config';

vi.mock('@/features/support/api/hooks/use-support-chat/useSupportChat');
vi.mock('@/features/phone-link/api/hooks/use-guardian-link/useGuardianLink');

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

it('should flag Phone link when notifications stopped', () => {
  phoneStatus({
    availability: 'ready',
    link: { ...activePhone, delivery: { state: 'terminal', reason: 'hook_not_found' } },
    offer: null
  });
  renderItems();

  expect(screen.getByRole('link', { name: 'Phone link, needs attention' })).toBeInTheDocument();
  expect(document.querySelectorAll('a [aria-hidden="true"]')).toHaveLength(1);
  expect(useGuardianLink).toHaveBeenCalledWith(GUARDIAN_LINK_NAV_POLL_MS);
});

it('should flag Phone link while the phone is not reached', () => {
  phoneStatus({
    availability: 'ready',
    link: { ...activePhone, delivery: { state: 'inactive', reason: null } },
    offer: null
  });
  renderItems();

  expect(screen.getByRole('link', { name: 'Phone link, needs attention' })).toBeInTheDocument();
});

it.each([
  ['a phone is active', { availability: 'ready', link: activePhone, offer: null }],
  ['no phone was ever linked', { availability: 'ready', link: null, offer: null }],
  ['this host cannot notify at all', { availability: 'no_push_gateway', link: null, offer: null }]
])('should not flag Phone link when %s', (_name, data) => {
  phoneStatus(data);
  renderItems();

  expect(screen.getByRole('link', { name: 'Phone link' })).toBeInTheDocument();
  expect(document.querySelector('a [aria-hidden="true"]')).toBeNull();
});

it.each([
  ['the policy keeps it off', { availability: 'disabled', link: null, offer: null }],
  ['the status has not loaded', undefined]
])('should hide Phone link when %s', (_name, data) => {
  phoneStatus(data);
  renderItems();

  expect(screen.queryByRole('link', { name: /Phone link/ })).toBeNull();
  expect(screen.getByRole('link', { name: 'Support' })).toBeInTheDocument();
});
