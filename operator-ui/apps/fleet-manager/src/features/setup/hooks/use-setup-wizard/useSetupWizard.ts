import { useState } from 'react';

export type SetupStep = 'doors' | 'phrase' | 'restore' | 'authorization' | 'price';

export const SETUP_STEP_LABELS = ['Start', 'Recovery phrase', 'Authorization', 'Price'];

// Which labelled step each screen sits under. The restore fork shares step 1 with
// the new-fleet phrase screen: both are "the phrase", read one way or written the
// other.
const stepIndexes: Record<SetupStep, number> = {
  doors: 0,
  phrase: 1,
  restore: 1,
  authorization: 2,
  price: 3
};

export interface SetupWizard {
  step: SetupStep;
  stepIndex: number;
  completedSteps: number[];
  onChooseNewFleet: () => void;
  onChooseRestore: () => void;
  onBackToDoors: () => void;
  onPhraseSaved: () => void;
  /** A recovery always continues to the authorization step. The daemon reports
   *  `waiting_for_authorization` immediately after a restore whether or not an
   *  authorization exists, so there is no value here worth branching on. */
  onRestored: () => void;
  onAuthorizationSettled: () => void;
}

export const useSetupWizard = (initialStep: SetupStep): SetupWizard => {
  const [step, setStep] = useState<SetupStep>(initialStep);
  const stepIndex = stepIndexes[step];

  return {
    step,
    stepIndex,
    completedSteps: Array.from({ length: stepIndex }, (_, index) => index),
    onChooseNewFleet: () => setStep('phrase'),
    onChooseRestore: () => setStep('restore'),
    onBackToDoors: () => setStep('doors'),
    onPhraseSaved: () => setStep('authorization'),
    onRestored: () => setStep('authorization'),
    onAuthorizationSettled: () => setStep('price')
  };
};
