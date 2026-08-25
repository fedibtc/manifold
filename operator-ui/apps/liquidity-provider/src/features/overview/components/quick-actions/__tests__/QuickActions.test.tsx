import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { describe, expect, it } from 'vitest';
import { QuickActions } from '../QuickActions';

const renderActions = () =>
  render(
    <MemoryRouter initialEntries={['/']}>
      <QuickActions />
    </MemoryRouter>
  );

describe('QuickActions', () => {
  it('should render the section title', () => {
    renderActions();

    expect(screen.getByRole('heading', { name: 'Quick actions' })).toBeTruthy();
  });

  it('should link each action to its destination', () => {
    renderActions();

    expect(screen.getByRole('link', { name: 'Funds' }).getAttribute('href')).toBe('/funds');
    expect(screen.getByRole('link', { name: 'Advertisement' }).getAttribute('href')).toBe(
      '/advertisement'
    );
    expect(screen.getByRole('link', { name: 'Allocations' }).getAttribute('href')).toBe(
      '/allocations'
    );
  });

  it('should not offer an action for a workflow the app cannot route to', () => {
    renderActions();

    expect(screen.queryByRole('link', { name: 'Requests' })).toBeNull();
  });
});
