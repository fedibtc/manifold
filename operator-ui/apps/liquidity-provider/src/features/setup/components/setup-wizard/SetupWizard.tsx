import { Button, Stepper } from '@operator-ui/common-ui';
import { type ReactNode, useState } from 'react';
import { ChainObserverStep } from '@/features/setup/components/steps/chain-observer-step/ChainObserverStep';
import { GatewayStep } from '@/features/setup/components/steps/gateway-step/GatewayStep';
import { NetworkStep } from '@/features/setup/components/steps/network-step/NetworkStep';
import { PolicyCapacityStep } from '@/features/setup/components/steps/policy-capacity-step/PolicyCapacityStep';
import { RelaysEndpointStep } from '@/features/setup/components/steps/relays-endpoint-step/RelaysEndpointStep';
import { ReviewStep } from '@/features/setup/components/steps/review-step/ReviewStep';
import { TrustStep } from '@/features/setup/components/steps/trust-step/TrustStep';
import { type ConfigDraft, initialDraft, STEP_LABELS } from '@/features/setup/services/draft';
import { STEP_VALIDATORS } from '@/features/setup/services/validation';
import type { StepProps } from '@/features/setup/types';
import styles from './SetupWizard.module.css';

const lastStep = STEP_LABELS.length - 1;

interface SetupWizardProps {
  trustPanel: ReactNode;
  /** Raised when the operator leaves the final "you're live" screen. */
  onComplete: () => void;
}

const renderStep = (
  step: number,
  props: StepProps,
  trustPanel: ReactNode,
  onComplete: () => void
) => {
  switch (step) {
    case 0:
      return <NetworkStep {...props} />;
    case 1:
      return <GatewayStep {...props} />;
    case 2:
      return <ChainObserverStep {...props} />;
    case 3:
      return <RelaysEndpointStep {...props} />;
    case 4:
      return <PolicyCapacityStep {...props} />;
    case 5:
      return <TrustStep>{trustPanel}</TrustStep>;
    default:
      return <ReviewStep {...props} onComplete={onComplete} />;
  }
};

export const SetupWizard = ({ trustPanel, onComplete }: SetupWizardProps) => {
  const [draft, setDraft] = useState<ConfigDraft>(initialDraft);
  const [step, setStep] = useState(0);
  const [errors, setErrors] = useState<Record<string, string>>({});

  const onChange = (patch: Partial<ConfigDraft>) => {
    setDraft((current) => ({ ...current, ...patch }));
  };
  const onBack = () => {
    setErrors({});
    setStep((current) => Math.max(0, current - 1));
  };
  const onNext = () => {
    const found = STEP_VALIDATORS[step](draft);
    if (Object.keys(found).length > 0) {
      setErrors(found);
      return;
    }
    setErrors({});
    setStep((current) => Math.min(lastStep, current + 1));
  };

  const completed = Array.from({ length: step }, (_, index) => index);
  const stepProps: StepProps = { draft, onChange, errors };
  return (
    <div className={styles.wrapper}>
      <header className={styles.header}>
        <h1 className={styles.title}>Setup — {STEP_LABELS[step]}</h1>

        <p className={styles.subtitle}>
          Step {step + 1} of {STEP_LABELS.length} · Configure your liquidity provider
        </p>
      </header>

      <Stepper steps={[...STEP_LABELS]} current={step} completed={completed} />

      {renderStep(step, stepProps, trustPanel, onComplete)}

      <div className={styles.actions}>
        {step > 0 ? (
          <Button variant="secondary" onClick={onBack}>
            Back
          </Button>
        ) : (
          <span />
        )}

        {step < lastStep ? (
          <Button variant="primary" onClick={onNext}>
            Continue
          </Button>
        ) : (
          <span />
        )}
      </div>
    </div>
  );
};
