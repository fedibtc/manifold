import { fireEvent, render, screen } from '@testing-library/react';
import { vi } from 'vitest';
import { KeyValueEditor } from '../KeyValueEditor';

it('should add an empty pair on Add', () => {
  const onChange = vi.fn();
  render(<KeyValueEditor pairs={[['region', 'eu']]} onChange={onChange} />);

  fireEvent.click(screen.getByRole('button', { name: 'Add' }));
  expect(onChange).toHaveBeenCalledWith([
    ['region', 'eu'],
    ['', '']
  ]);
});

it('should remove a pair on the row remove button', () => {
  const onChange = vi.fn();
  render(
    <KeyValueEditor
      pairs={[
        ['region', 'eu'],
        ['tier', 'gold']
      ]}
      onChange={onChange}
    />
  );

  fireEvent.click(screen.getByRole('button', { name: 'Remove pair 1' }));
  expect(onChange).toHaveBeenCalledWith([['tier', 'gold']]);
});

it('should call onChange with the updated pairs when a value changes', () => {
  const onChange = vi.fn();
  render(<KeyValueEditor pairs={[['region', 'eu']]} onChange={onChange} />);

  fireEvent.change(screen.getByLabelText('Value 1'), { target: { value: 'us' } });
  expect(onChange).toHaveBeenCalledWith([['region', 'us']]);
});
