import { fireEvent, render, screen } from '@testing-library/react';
import { vi } from 'vitest';
import { SelectField } from '../SelectField';

const options = [
  { value: 'mainnet', label: 'Mainnet' },
  { value: 'signet', label: 'Signet' }
];

it('should render options and call onChange on select', () => {
  const onChange = vi.fn();
  render(<SelectField label="Network" value="mainnet" onChange={onChange} options={options} />);

  expect(screen.getByRole('option', { name: 'Mainnet' })).toBeInTheDocument();
  expect(screen.getByRole('option', { name: 'Signet' })).toBeInTheDocument();

  fireEvent.change(screen.getByLabelText('Network'), { target: { value: 'signet' } });
  expect(onChange).toHaveBeenCalledWith('signet');
});

it('should associate the hint text with the select via aria-describedby', () => {
  render(
    <SelectField
      label="Network"
      value="mainnet"
      onChange={vi.fn()}
      options={options}
      hint="Pick a network"
    />
  );

  const select = screen.getByLabelText('Network');
  const hint = screen.getByText('Pick a network');
  expect(select.getAttribute('aria-describedby')).toBe(hint.id);
});

it('should associate the error text with the select and mark it invalid', () => {
  render(
    <SelectField
      label="Network"
      value="mainnet"
      onChange={vi.fn()}
      options={options}
      error="Required"
    />
  );

  const select = screen.getByLabelText('Network');
  const error = screen.getByRole('alert');
  expect(error).toHaveTextContent('Required');
  expect(select.getAttribute('aria-describedby')).toBe(error.id);
  expect(select).toHaveAttribute('aria-invalid', 'true');
});
