import { render, screen } from '@testing-library/react';
import { SectionDescription } from '../SectionDescription';

it('should render the description text under a section title', () => {
  render(<SectionDescription>Ongoing fees from federations you guard.</SectionDescription>);

  expect(screen.getByText('Ongoing fees from federations you guard.')).toBeInTheDocument();
});
