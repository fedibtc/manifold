import { fireEvent, render, screen } from '@testing-library/react';
import { useState } from 'react';
import { describe, expect, it } from 'vitest';
import { RelaysEndpointStep } from '@/features/setup/components/steps/relays-endpoint-step/RelaysEndpointStep';
import { type ConfigDraft, initialDraft } from '@/features/setup/services/draft';
import { validateRelaysEndpoint } from '@/features/setup/services/validation';

const Harness = () => {
  const [draft, setDraft] = useState<ConfigDraft>(initialDraft);
  const onChange = (patch: Partial<ConfigDraft>) => setDraft((d) => ({ ...d, ...patch }));
  const errors = validateRelaysEndpoint(draft);
  return <RelaysEndpointStep draft={draft} onChange={onChange} errors={errors} />;
};

describe('RelaysEndpointStep', () => {
  it('should add a relay row, flag a non-wss entry, then remove the row', () => {
    render(<Harness />);
    expect(screen.queryByLabelText('Relay 1')).toBeNull();

    fireEvent.click(screen.getByText('Add relay'));
    const relay = screen.getByLabelText('Relay 1') as HTMLInputElement;
    expect(relay).toBeTruthy();

    fireEvent.change(relay, { target: { value: 'http://relay.example.com' } });
    expect(screen.getByText('Every relay must start with wss://.')).toBeTruthy();

    fireEvent.change(relay, { target: { value: 'wss://relay.example.com' } });
    expect(screen.queryByText('Every relay must start with wss://.')).toBeNull();

    fireEvent.click(screen.getByLabelText('Remove relay 1'));
    expect(screen.queryByLabelText('Relay 1')).toBeNull();
  });
});
