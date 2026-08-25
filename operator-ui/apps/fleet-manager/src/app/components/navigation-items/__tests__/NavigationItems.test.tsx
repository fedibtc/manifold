import { render, screen } from '@testing-library/react';
import { createMemoryRouter, RouterProvider } from 'react-router-dom';
import { NavigationItems } from '../NavigationItems';
import { NAV_ITEMS } from '../nav-config';

it('should render a link for every configured nav item', () => {
  const router = createMemoryRouter([{ path: '*', element: <NavigationItems /> }]);
  render(<RouterProvider router={router} />);

  for (const item of NAV_ITEMS) {
    screen.getByRole('link', { name: item.label });
  }
});
