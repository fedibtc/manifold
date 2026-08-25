import { fireEvent, render, screen } from '@testing-library/react';
import { useState } from 'react';
import { describe, expect, it } from 'vitest';
import { NetworkStep } from '@/features/setup/components/steps/network-step/NetworkStep';
import { type ConfigDraft, initialDraft } from '@/features/setup/services/draft';

const Harness = () => {
  const [draft, setDraft] = useState<ConfigDraft>(initialDraft);
  const onChange = (patch: Partial<ConfigDraft>) => setDraft((d) => ({ ...d, ...patch }));
  return <NetworkStep draft={draft} onChange={onChange} errors={{}} />;
};

describe('NetworkStep', () => {
  it('should offer every supported Bitcoin network', () => {
    render(<Harness />);

    const select = screen.getByLabelText('Network') as HTMLSelectElement;
    const values = [...select.options].map((option) => option.value);
    expect(values).toEqual(['signet', 'bitcoin', 'testnet', 'regtest']);
  });

  it('should update draft.network on selection', () => {
    render(<Harness />);

    const select = screen.getByLabelText('Network') as HTMLSelectElement;
    fireEvent.change(select, { target: { value: 'regtest' } });

    expect(select.value).toBe('regtest');
  });

  // The list gates every public request the daemon accepts and every
  // advertisement an FI keeps. It has no editor: it used to be written as a
  // side effect of editing the accepted attesters, on another step, so changing
  // the network alone left the provider advertising one network and serving
  // another.
  it('should derive policy.supported_networks from the selected network', () => {
    render(<Harness />);

    const select = screen.getByLabelText('Network') as HTMLSelectElement;
    fireEvent.change(select, { target: { value: 'regtest' } });

    expect(screen.getByText('Serving: regtest')).toBeTruthy();
  });

  it('should warn that saving re-validates the network against the gateway', () => {
    render(<Harness />);

    expect(screen.getByText('Network is checked against your gateway')).toBeTruthy();
  });

  it('should surface a network validation error', () => {
    render(
      <NetworkStep draft={initialDraft} onChange={() => {}} errors={{ network: 'Pick one.' }} />
    );

    expect(screen.getByText('Pick one.')).toBeTruthy();
  });
});
