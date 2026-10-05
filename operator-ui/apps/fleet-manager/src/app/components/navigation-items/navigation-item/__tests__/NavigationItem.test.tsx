import { render, screen } from '@testing-library/react';
import { createMemoryRouter, RouterProvider } from 'react-router-dom';
import { NavigationItem } from '../NavigationItem';

const renderAt = (initialPath: string) => {
  const router = createMemoryRouter(
    [
      {
        path: '*',
        element: <NavigationItem item={{ key: 'seats', label: 'Seats', path: '/seats' }} />
      }
    ],
    { initialEntries: [initialPath] }
  );
  return render(<RouterProvider router={router} />);
};

it('should render the item label as a link', () => {
  renderAt('/seats');

  screen.getByRole('link', { name: 'Seats' });
});

it('should link to the item path', () => {
  renderAt('/overview');

  const link = screen.getByRole('link', { name: 'Seats' });
  expect(link.getAttribute('href')).toBe('/seats');
});

it('should mark unread items with a dot and name the count', () => {
  const router = createMemoryRouter([
    {
      path: '*',
      element: (
        <NavigationItem item={{ key: 'support', label: 'Support', path: '/support' }} count={3} />
      )
    }
  ]);
  render(<RouterProvider router={router} />);

  const link = screen.getByRole('link', { name: 'Support, 3 unread' });
  expect(link).toHaveTextContent(/^Support$/);
  expect(link.querySelector('[aria-hidden="true"]')).not.toBeNull();
});
