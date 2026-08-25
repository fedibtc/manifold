import { fireEvent, render, screen } from '@testing-library/react';
import { useState } from 'react';
import { describe, expect, it } from 'vitest';
import { PolicyCapacityStep } from '@/features/setup/components/steps/policy-capacity-step/PolicyCapacityStep';
import { type ConfigDraft, initialDraft } from '@/features/setup/services/draft';
import { validatePolicyCapacity } from '@/features/setup/services/validation';

const Harness = () => {
  const [draft, setDraft] = useState<ConfigDraft>(initialDraft);
  const onChange = (patch: Partial<ConfigDraft>) => setDraft((d) => ({ ...d, ...patch }));
  return <PolicyCapacityStep draft={draft} onChange={onChange} errors={{}} />;
};

describe('PolicyCapacityStep', () => {
  it('should toggle a supported source on and off', () => {
    render(<Harness />);
    const gateway = screen.getByLabelText('Gateway') as HTMLInputElement;
    expect(gateway.checked).toBe(false);

    fireEvent.click(gateway);
    expect((screen.getByLabelText('Gateway') as HTMLInputElement).checked).toBe(true);

    fireEvent.click(screen.getByLabelText('Gateway'));
    expect((screen.getByLabelText('Gateway') as HTMLInputElement).checked).toBe(false);
  });

  it('should add an attester row and remove it', () => {
    render(<Harness />);
    expect(screen.queryByLabelText('Attester 1 pubkey')).toBeNull();

    fireEvent.click(screen.getByText('Add attester'));
    const input = screen.getByLabelText('Attester 1 pubkey') as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'npub-abc' } });
    expect((screen.getByLabelText('Attester 1 pubkey') as HTMLInputElement).value).toBe('npub-abc');

    fireEvent.click(screen.getByLabelText('Remove attester 1'));
    expect(screen.queryByLabelText('Attester 1 pubkey')).toBeNull();
  });

  it('should show a cap field only in explicit_cap mode', () => {
    render(<Harness />);
    expect(screen.queryByLabelText('Cap amount (SATS)')).toBeNull();

    fireEvent.change(screen.getByLabelText('Capacity mode'), { target: { value: 'explicit_cap' } });
    expect(screen.getByLabelText('Cap amount (SATS)')).toBeTruthy();
  });
});

describe('validatePolicyCapacity', () => {
  it('should reject an empty supported_sources list', () => {
    const errors = validatePolicyCapacity({
      ...initialDraft,
      capacity: { mode: 'available_funds', supported_sources: [] },
      policy: {
        accepted_attester_policies: [
          { attester_pubkey: 'npub-1', verification_requirement: 'all_trusted' }
        ],
        supported_networks: ['signet']
      }
    });
    expect(errors.supported_sources).toBeTruthy();
    expect(errors.accepted_attester_policies).toBeUndefined();
  });

  it('should reject when no attester with a pubkey is present', () => {
    const errors = validatePolicyCapacity({
      ...initialDraft,
      capacity: { mode: 'available_funds', supported_sources: ['gateway'] },
      policy: {
        accepted_attester_policies: [
          { attester_pubkey: '   ', verification_requirement: 'all_trusted' }
        ],
        supported_networks: ['signet']
      }
    });
    expect(errors.accepted_attester_policies).toBeTruthy();
    expect(errors.supported_sources).toBeUndefined();
  });

  it('should reject an explicit_cap of zero', () => {
    const errors = validatePolicyCapacity({
      ...initialDraft,
      capacity: { mode: 'explicit_cap', explicit_cap: 0, supported_sources: ['gateway'] },
      policy: {
        accepted_attester_policies: [
          { attester_pubkey: 'npub-1', verification_requirement: 'all_trusted' }
        ],
        supported_networks: ['signet']
      }
    });
    expect(errors.explicit_cap).toBeTruthy();
  });

  it('should pass a fully specified draft', () => {
    const errors = validatePolicyCapacity({
      ...initialDraft,
      capacity: { mode: 'explicit_cap', explicit_cap: 1000, supported_sources: ['gateway'] },
      policy: {
        accepted_attester_policies: [
          { attester_pubkey: 'npub-1', verification_requirement: 'all_trusted' }
        ],
        supported_networks: ['signet']
      }
    });
    expect(Object.keys(errors)).toHaveLength(0);
  });
});
