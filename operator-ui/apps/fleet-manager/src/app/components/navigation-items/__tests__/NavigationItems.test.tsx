import { render, screen } from '@testing-library/react';
import { createMemoryRouter, RouterProvider } from 'react-router-dom';
import { beforeEach, vi } from 'vitest';
import { useSupportChat } from '@/features/support/api/hooks/use-support-chat/useSupportChat';
import { SUPPORT_UNREAD_POLL_MS } from '@/shared/api/pollingIntervals';
import { NavigationItems } from '../NavigationItems';
import { NAV_ITEMS } from '../nav-config';

vi.mock('@/features/support/api/hooks/use-support-chat/useSupportChat');

const unread = (count: number | undefined) =>
  vi.mocked(useSupportChat).mockReturnValue({
    data: count === undefined ? undefined : { available: true, messages: [], unread: count }
  } as ReturnType<typeof useSupportChat>);

const renderItems = () => {
  const router = createMemoryRouter([{ path: '*', element: <NavigationItems /> }]);
  render(<RouterProvider router={router} />);
};

beforeEach(() => unread(undefined));

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
