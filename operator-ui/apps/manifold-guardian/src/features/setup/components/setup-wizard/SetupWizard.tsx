import { Stepper } from '@operator-ui/common-ui';
import { SetupAuthorization } from '@/features/setup/components/setup-authorization/SetupAuthorization';
import { SetupDoors } from '@/features/setup/components/setup-doors/SetupDoors';
import { SetupPhrase } from '@/features/setup/components/setup-phrase/SetupPhrase';
import { SetupPrice } from '@/features/setup/components/setup-price/SetupPrice';
import { SetupRestore } from '@/features/setup/components/setup-restore/SetupRestore';
import { SetupTerms } from '@/features/setup/components/setup-terms/SetupTerms';
import {
  BADGELESS_SETUP_STEP_LABELS,
  SETUP_STEP_LABELS,
  useSetupWizard
} from '@/features/setup/hooks/use-setup-wizard/useSetupWizard';
import { useOnboarding } from '@/shared/api/hooks/use-onboarding/useOnboarding';
import { isBadgeless } from '@/shared/utils/authorization';
import styles from './SetupWizard.module.css';

interface SetupWizardProps {
  onComplete: () => void;
  initialStep: 'doors' | 'authorization' | 'terms';
}

export const SetupWizard = ({ onComplete, initialStep }: SetupWizardProps) => {
  const wizard = useSetupWizard(initialStep);
  const onboarding = useOnboarding();
  // Past the authorization stage with no authorization means the operator
  // chose to set up without a badge; before it, nothing is decided yet.
  const withoutBadge = onboarding.data?.stage === 'initial_offer' && isBadgeless(onboarding.data);

  return (
    <div className={styles.root}>
      <div className={styles.panel}>
        <Stepper
          steps={withoutBadge ? BADGELESS_SETUP_STEP_LABELS : SETUP_STEP_LABELS}
          current={wizard.stepIndex}
          completed={wizard.completedSteps}
        />
        {wizard.step === 'doors' && (
          <SetupDoors onNewFleet={wizard.onChooseNewFleet} onRestore={wizard.onChooseRestore} />
        )}
        {wizard.step === 'phrase' && <SetupPhrase onSaved={wizard.onPhraseSaved} />}
        {wizard.step === 'restore' && (
          <SetupRestore onRestored={wizard.onRestored} onCancel={wizard.onBackToDoors} />
        )}
        {wizard.step === 'authorization' && (
          <SetupAuthorization onSettled={wizard.onAuthorizationSettled} />
        )}
        {wizard.step === 'terms' && <SetupTerms onAccepted={wizard.onTermsAccepted} />}
        {wizard.step === 'price' && <SetupPrice withPrice={!withoutBadge} onDone={onComplete} />}
      </div>
    </div>
  );
};
