import { fireEvent, render, screen } from '@testing-library/react';
import { vi } from 'vitest';
import { TextInput } from '../TextInput';

it('should call onChange with the typed value', () => {
  const onChange = vi.fn();
  render(<TextInput label="Alias" value="" onChange={onChange} />);

  fireEvent.change(screen.getByLabelText('Alias'), { target: { value: 'flip-1' } });
  expect(onChange).toHaveBeenCalledWith('flip-1');
});

it('should show the error text and error border when error is set', () => {
  render(<TextInput label="Alias" value="" onChange={vi.fn()} error="Required" />);

  expect(screen.getByText('Required')).toBeInTheDocument();
  expect(screen.getByLabelText('Alias')).toHaveAttribute('data-invalid', 'true');
});

it('should render type password when secret', () => {
  render(<TextInput label="Secret" value="hunter2" onChange={vi.fn()} type="password" />);

  expect(screen.getByLabelText('Secret')).toHaveAttribute('type', 'password');
});

it('should associate the hint text with the input via aria-describedby', () => {
  render(<TextInput label="Alias" value="" onChange={vi.fn()} hint="Shown to guardians" />);

  const input = screen.getByLabelText('Alias');
  const hint = screen.getByText('Shown to guardians');
  expect(input.getAttribute('aria-describedby')).toBe(hint.id);
});

it('should associate the error text with the input and mark it invalid', () => {
  render(<TextInput label="Alias" value="" onChange={vi.fn()} error="Required" />);

  const input = screen.getByLabelText('Alias');
  const error = screen.getByRole('alert');
  expect(error).toHaveTextContent('Required');
  expect(input.getAttribute('aria-describedby')).toBe(error.id);
  expect(input).toHaveAttribute('aria-invalid', 'true');
});

it('should not set aria-invalid when there is no error', () => {
  render(<TextInput label="Alias" value="" onChange={vi.fn()} />);

  expect(screen.getByLabelText('Alias')).toHaveAttribute('aria-invalid', 'false');
});
