import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { TrustStep } from '@/features/setup/components/steps/trust-step/TrustStep';
import { initialDraft } from '@/features/setup/services/draft';
import { STEP_VALIDATORS } from '@/features/setup/services/validation';

describe('TrustStep', () => {
  it('should render the Trust heading and its children', () => {
    render(
      <TrustStep>
        <p>Attestation panel</p>
      </TrustStep>
    );
    expect(screen.getByRole('heading', { name: 'Trust' })).toBeTruthy();
    expect(screen.getByText('Attestation panel')).toBeTruthy();
  });

  it('should be skippable — its validator returns no errors', () => {
    expect(STEP_VALIDATORS[5](initialDraft)).toEqual({});
  });
});
