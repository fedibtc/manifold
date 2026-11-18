import { fireEvent, render, screen } from '@testing-library/react';
import { createMemoryRouter, RouterProvider } from 'react-router-dom';
import { beforeEach, expect, it } from 'vitest';
import { WhatsNew } from '@/shared/components/whats-new/WhatsNew';
import { useWhatsNewSeen } from '@/shared/whats-new/whatsNew';

const item = (id: string) => ({
  id,
  title: `Title ${id}`,
  body: `Body ${id}`,
  action: { label: `Open ${id}`, path: `/${id}` }
});

// Drops what has been marked seen, as the real item hooks do.
const Items = () => {
  const seenA = useWhatsNewSeen('a');
  const seenB = useWhatsNewSeen('b');
  return <WhatsNew items={[!seenA && item('a'), !seenB && item('b')].filter((x) => x !== false)} />;
};

beforeEach(() => window.localStorage.clear());

it('should stack every item and drop only the one set aside', () => {
  render(<RouterProvider router={createMemoryRouter([{ path: '*', element: <Items /> }])} />);

  expect(screen.getByRole('link', { name: 'Open a' })).toHaveAttribute('href', '/a');
  expect(screen.getByRole('link', { name: 'Open b' })).toHaveAttribute('href', '/b');

  fireEvent.click(screen.getAllByRole('button', { name: 'Not now' })[1]);
  expect(screen.getByText('Title a')).toBeInTheDocument();
  expect(screen.queryByText('Title b')).not.toBeInTheDocument();
  expect(window.localStorage.getItem('manifold-guardian.whats-new-seen')).toBe('["b"]');
});
