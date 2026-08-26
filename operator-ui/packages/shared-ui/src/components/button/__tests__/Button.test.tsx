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

// A short request repainted the button in the inactive variant and back, which
// reads as a flicker. Busy is a state of this button, not a different button.
it('should keep its own variant while loading', () => {
  render(
    <Button variant="secondary" loading>
      Check now
    </Button>
  );

  const button = screen.getByRole('button', { name: 'Check now' });
  expect(button).toHaveAttribute('data-variant', 'secondary');
  expect(button).toHaveAttribute('aria-busy', 'true');
});

it('should not fire onClick while loading', () => {
  const onClick = vi.fn();
  render(
    <Button loading onClick={onClick}>
      Check now
    </Button>
  );

  const button = screen.getByRole('button', { name: 'Check now' });
  expect(button).toBeDisabled();

  fireEvent.click(button);
  expect(onClick).not.toHaveBeenCalled();
});

// The label stays in the box while the spinner is over it, so the button cannot
// change width for the duration of the request.
it('should keep the label in the tree while loading', () => {
  render(<Button loading>Check now</Button>);

  expect(screen.getByText('Check now')).toBeInTheDocument();
});
