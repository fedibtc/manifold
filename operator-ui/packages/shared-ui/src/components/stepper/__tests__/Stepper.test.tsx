import { render, screen } from '@testing-library/react';
import { Stepper } from '../Stepper';

const steps = ['Identity', 'Gateway', 'Review'];

it('should mark the current step active and completed steps done', () => {
  render(<Stepper steps={steps} current={1} completed={[0]} />);

  expect(screen.getByText('Identity')).toHaveAttribute('data-state', 'completed');
  expect(screen.getByText('Gateway')).toHaveAttribute('data-state', 'current');
  expect(screen.getByText('Review')).toHaveAttribute('data-state', 'upcoming');
});

it('should apply the active pill styling to the current step', () => {
  render(<Stepper steps={steps} current={0} />);

  expect(screen.getByText('Identity')).toHaveAttribute('data-state', 'current');
});

it('should render the steps as a list with aria-current on the active step', () => {
  render(<Stepper steps={steps} current={1} completed={[0]} />);

  const list = screen.getByRole('list');
  const items = screen.getAllByRole('listitem');
  expect(list).toBeInTheDocument();
  expect(items).toHaveLength(3);
  expect(items[1]).toHaveAttribute('aria-current', 'step');
  expect(items[0]).not.toHaveAttribute('aria-current');
  expect(items[2]).not.toHaveAttribute('aria-current');
});

it('should expose a visually-hidden status word per step', () => {
  render(<Stepper steps={steps} current={1} completed={[0]} />);

  expect(screen.getByText('completed', { exact: false })).toBeInTheDocument();
  expect(screen.getByText('current', { exact: false })).toBeInTheDocument();
  expect(screen.getByText('upcoming', { exact: false })).toBeInTheDocument();
});
