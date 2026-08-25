import { fireEvent, render, screen } from '@testing-library/react';
import { ScenarioToggle } from '../ScenarioToggle';

const entry = { name: 'populated', desc: 'One running seat.', affects: ['seats'] };

it('should pass its scenario name to the select handler', () => {
  const selected: string[] = [];
  render(
    <ul>
      <ScenarioToggle entry={entry} isActive={false} onSelect={(name) => selected.push(name)} />
    </ul>
  );

  fireEvent.click(screen.getByRole('switch', { name: /populated/i }));

  expect(selected).toEqual(['populated']);
});

it('should read as off when the scenario is not active', () => {
  render(
    <ul>
      <ScenarioToggle entry={entry} isActive={false} onSelect={() => undefined} />
    </ul>
  );

  expect(screen.getByRole('switch', { name: /populated/i })).toHaveAttribute(
    'aria-checked',
    'false'
  );
});

it('should read as on when the scenario is active', () => {
  render(
    <ul>
      <ScenarioToggle entry={entry} isActive onSelect={() => undefined} />
    </ul>
  );

  expect(screen.getByRole('switch', { name: /populated/i })).toHaveAttribute(
    'aria-checked',
    'true'
  );
});

it('should render the scenario description', () => {
  render(
    <ul>
      <ScenarioToggle entry={entry} isActive={false} onSelect={() => undefined} />
    </ul>
  );

  expect(screen.getByText('One running seat.')).toBeInTheDocument();
});
