import { render, screen } from '@testing-library/react';
import { Banner } from '../Banner';

it('should render the variant styling and title', () => {
  render(
    <Banner variant="error" title="Setup failed">
      Guardian is unreachable.
    </Banner>
  );

  expect(screen.getByText('Setup failed')).toBeInTheDocument();
  expect(screen.getByText('Guardian is unreachable.')).toBeInTheDocument();

  const banner = screen.getByText('Guardian is unreachable.').closest('[data-variant]');
  expect(banner).toHaveAttribute('data-variant', 'error');
});
