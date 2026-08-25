import { render, screen } from '@testing-library/react';
import { FormField } from '../FormField';

it('should render the label, hint, and error text', () => {
  render(
    <FormField label="Alias" hint="Shown to guardians">
      {() => <input aria-label="Alias entry" />}
    </FormField>
  );

  expect(screen.getByText('Alias')).toBeInTheDocument();
  expect(screen.getByText('Shown to guardians')).toBeInTheDocument();
});

it('should name the group with its label so the whole control set is announced', () => {
  render(<FormField label="Relays">{() => <input aria-label="Relay 1" />}</FormField>);

  expect(screen.getByRole('group', { name: 'Relays' })).toBeInTheDocument();
});

it('should describe the group with its hint', () => {
  render(
    <FormField label="Relays" hint="One relay per row.">
      {() => <input aria-label="Relay 1" />}
    </FormField>
  );

  const group = screen.getByRole('group', { name: 'Relays' });
  const hint = screen.getByText('One relay per row.');
  expect(group.getAttribute('aria-describedby')).toBe(hint.id);
});

it('should describe the group with its error instead of its hint once errored', () => {
  render(
    <FormField label="Relays" hint="One relay per row." error="Add at least one relay.">
      {() => <input aria-label="Relay 1" />}
    </FormField>
  );

  const group = screen.getByRole('group', { name: 'Relays' });
  const error = screen.getByRole('alert');
  expect(error).toHaveTextContent('Add at least one relay.');
  expect(group.getAttribute('aria-describedby')).toBe(error.id);
  expect(screen.queryByText('One relay per row.')).not.toBeInTheDocument();
});

it('should pass the hint id to children via the describedBy render-prop argument', () => {
  render(
    <FormField label="Alias" hint="Shown to guardians">
      {({ describedBy }) => <input aria-describedby={describedBy} data-testid="input" />}
    </FormField>
  );

  const input = screen.getByTestId('input');
  const hint = screen.getByText('Shown to guardians');
  expect(input.getAttribute('aria-describedby')).toBe(hint.id);
});
