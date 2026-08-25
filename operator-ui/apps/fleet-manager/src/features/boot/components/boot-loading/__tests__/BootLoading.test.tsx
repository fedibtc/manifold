import { render, screen } from '@testing-library/react';
import { BootLoading } from '../BootLoading';

it('should render a loading message', () => {
  render(<BootLoading />);

  screen.getByText('Loading…');
});
