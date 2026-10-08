import { render, screen } from '@testing-library/react';
import { FleetStarting } from '../FleetStarting';

it('should name the fleet as starting, not as needing setup', () => {
  render(<FleetStarting />);

  screen.getByRole('heading', { name: 'Manifold Fedimint Guardian is starting' });
});

it('should say the guardians and seats are safe while the fleet starts', () => {
  render(<FleetStarting />);

  screen.getByText(
    'Your guardians and seats are safe. The dashboard opens when this host is ready.'
  );
});

it('should show that the fleet is still starting', () => {
  render(<FleetStarting />);

  screen.getByRole('status', { name: 'Starting' });
});

it('should offer no control, because nothing an operator does shortens the wait', () => {
  render(<FleetStarting />);

  expect(screen.queryByRole('button')).toBeNull();
});
