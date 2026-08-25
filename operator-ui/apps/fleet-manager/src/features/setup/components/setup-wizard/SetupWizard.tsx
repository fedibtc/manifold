import { Stepper } from '@operator-ui/common-ui';
import { SetupAuthorization } from '@/features/setup/components/setup-authorization/SetupAuthorization';
import { SetupDoors } from '@/features/setup/components/setup-doors/SetupDoors';
import { SetupPhrase } from '@/features/setup/components/setup-phrase/SetupPhrase';
import { SetupPrice } from '@/features/setup/components/setup-price/SetupPrice';
import { SetupRestore } from '@/features/setup/components/setup-restore/SetupRestore';
import {
  SETUP_STEP_LABELS,
  useSetupWizard
} from '@/features/setup/hooks/use-setup-wizard/useSetupWizard';
import styles from './SetupWizard.module.css';

interface SetupWizardProps {
  onComplete: () => void;
  initialStep: 'doors' | 'authorization' | 'price';
}

export const SetupWizard = ({ onComplete, initialStep }: SetupWizardProps) => {
  const wizard = useSetupWizard(initialStep);

  return (
    <div className={styles.root}>
      <div className={styles.panel}>
        <Stepper
          steps={SETUP_STEP_LABELS}
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
        {wizard.step === 'price' && <SetupPrice onDone={onComplete} />}
      </div>
    </div>
  );
};
