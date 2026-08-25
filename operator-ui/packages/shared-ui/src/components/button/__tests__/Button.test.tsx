import { fireEvent, render, screen } from '@testing-library/react';
import { vi } from 'vitest';
import { Button } from '../Button';

it('should render the label', () => {
  render(<Button>Continue</Button>);

  expect(screen.getByRole('button', { name: 'Continue' })).toBeInTheDocument();
});

it('should be disabled and not fire onClick when disabled', () => {
  const onClick = vi.fn();
  render(
    <Button disabled onClick={onClick}>
      Save
    </Button>
  );

  const button = screen.getByRole('button', { name: 'Save' });
  expect(button).toBeDisabled();

  fireEvent.click(button);
  expect(onClick).not.toHaveBeenCalled();
});

it('should expose the secondary variant on the element', () => {
  render(<Button variant="secondary">Back</Button>);

  const button = screen.getByRole('button', { name: 'Back' });
  expect(button).toHaveAttribute('data-variant', 'secondary');
});

it('should mark an inactive variant when disabled', () => {
  render(<Button disabled>Save</Button>);

  const button = screen.getByRole('button', { name: 'Save' });
  expect(button).toHaveAttribute('data-variant', 'inactive');
});
