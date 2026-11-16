import { fireEvent, render, screen } from '@testing-library/react';
import { vi } from 'vitest';
import { RadioGroupField } from '../RadioGroupField';

const OPTIONS = [
  { value: 'no', label: 'No monthly limit' },
  { value: 'yes', label: 'Yes, my plan has a monthly limit' }
] as const;

it('should preselect nothing and report the chosen option', () => {
  const onChange = vi.fn();
  render(<RadioGroupField label="Limited?" options={OPTIONS} value={null} onChange={onChange} />);

  const group = screen.getByRole('radiogroup', { name: 'Limited?' });
  expect(group).toBeInTheDocument();
  for (const radio of screen.getAllByRole('radio')) expect(radio).not.toBeChecked();

  fireEvent.click(screen.getByLabelText('Yes, my plan has a monthly limit'));
  expect(onChange).toHaveBeenCalledWith('yes');
});

it('should check the current value and describe the group by its hint and error', () => {
  render(
    <RadioGroupField
      label="Limited?"
      options={OPTIONS}
      value="no"
      onChange={() => {}}
      hint="A hint"
      error="Choose one."
    />
  );

  expect(screen.getByLabelText('No monthly limit')).toBeChecked();
  expect(screen.getByLabelText('Yes, my plan has a monthly limit')).not.toBeChecked();
  const group = screen.getByRole('radiogroup', { name: 'Limited?' });
  // The hint says how to answer, so the error joins it rather than replacing it.
  expect(group).toHaveAccessibleDescription('A hint Choose one.');
  expect(screen.getByText('A hint')).toBeVisible();
  expect(screen.getByText('A hint').nextElementSibling).toBe(screen.getByRole('alert'));
});

it('should move focus to the first option when an unanswered group gets an error', () => {
  const props = { label: 'Limited?', options: OPTIONS, value: null, onChange: () => {} };
  const { rerender } = render(
    <>
      <button type="button">Finish setup</button>

      <RadioGroupField {...props} hint="A hint" />
    </>
  );
  screen.getByRole('button', { name: 'Finish setup' }).focus();

  rerender(
    <>
      <button type="button">Finish setup</button>

      <RadioGroupField {...props} hint="A hint" error="Choose one." />
    </>
  );

  expect(screen.getByLabelText('No monthly limit')).toHaveFocus();
});

it('should move focus to the chosen option, not the first, on an error', () => {
  const { rerender } = render(
    <RadioGroupField label="Limited?" options={OPTIONS} value="yes" onChange={() => {}} />
  );

  rerender(
    <RadioGroupField
      label="Limited?"
      options={OPTIONS}
      value="yes"
      onChange={() => {}}
      error="That did not save."
    />
  );

  expect(screen.getByLabelText('Yes, my plan has a monthly limit')).toHaveFocus();
});

it('should leave focus alone while there is no error', () => {
  const field = (hint: string) => (
    <>
      <button type="button">Elsewhere</button>

      <RadioGroupField
        label="Limited?"
        options={OPTIONS}
        value={null}
        onChange={() => {}}
        hint={hint}
      />
    </>
  );
  const { rerender } = render(field('A hint'));
  screen.getByRole('button', { name: 'Elsewhere' }).focus();

  rerender(field('Another hint'));

  expect(screen.getByRole('button', { name: 'Elsewhere' })).toHaveFocus();
});
