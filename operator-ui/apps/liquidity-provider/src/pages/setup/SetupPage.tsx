import { AttestationPanel } from '@/features/attestations/components/attestation-panel/AttestationPanel';
import { SetupWizard } from '@/features/setup/components/setup-wizard/SetupWizard';

interface SetupPageProps {
  /** Closes the setup gate. Called when the operator leaves the "you're live"
   *  screen — the wizard, not the setup status, decides when setup is over. */
  onComplete: () => void;
}

export const SetupPage = ({ onComplete }: SetupPageProps) => (
  <SetupWizard trustPanel={<AttestationPanel />} onComplete={onComplete} />
);
