import { Banner, Button, truncateMiddle } from '@operator-ui/common-ui';
import type {
  ApplySetupConfigRequest,
  ApplySetupConfigResponse,
  SetupValidationCheck,
  SetupValidationSummary,
  ValidateSetupRequest,
  ValidateSetupResponse
} from '@operator-ui/types';
import { useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { STEP_VALIDATORS } from '@/features/setup/services/validation';
import type { StepProps } from '@/features/setup/types';
import { adminCall } from '@/shared/api/adminCall';
import { AdminApiError, AuthError, NetworkError, RouteDeferredError } from '@/shared/api/errors';
import { SETUP_STATE_KEY } from '@/shared/api/hooks/use-setup-state/useSetupState';
import { storeDraftSecrets, toSetupConfig } from '@/shared/config/secrets';
import styles from './ReviewStep.module.css';

type Phase = 'idle' | 'validating' | 'applying' | 'applied' | 'soft_fail' | 'error';

interface Notice {
  variant: 'error' | 'info';
  message: string;
}

const REQUIRED_STEPS = 5;

const isPass = (check: SetupValidationCheck): boolean => check.status === 'passed';

const draftComplete = (draft: StepProps['draft']): boolean =>
  Array.from({ length: REQUIRED_STEPS }, (_, index) => index).every(
    (index) => Object.keys(STEP_VALIDATORS[index](draft)).length === 0
  );

interface ReviewStepProps extends StepProps {
  /** Closes the setup gate once the operator leaves the live screen. */
  onComplete: () => void;
}

export const ReviewStep = ({ draft, onComplete }: ReviewStepProps) => {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const [phase, setPhase] = useState<Phase>('idle');
  const [validation, setValidation] = useState<SetupValidationSummary | null>(null);
  const [notice, setNotice] = useState<Notice | null>(null);

  const handleError = (error: unknown) => {
    if (error instanceof AuthError) {
      throw error;
    }
    if (error instanceof RouteDeferredError) {
      setNotice({ variant: 'info', message: 'This action is not available yet.' });
    } else if (error instanceof NetworkError) {
      setNotice({ variant: 'error', message: "Couldn't reach the daemon. Check it is running." });
    } else if (error instanceof AdminApiError) {
      setNotice({ variant: 'error', message: error.message });
    } else {
      setNotice({ variant: 'error', message: 'Something went wrong. Try again.' });
    }
    setPhase('error');
  };

  const runValidation = async () => {
    setPhase('validating');
    setNotice(null);
    try {
      // The daemon validates a candidate against the stored secrets, so the
      // credential the operator typed two steps ago has to be stored before it
      // can be tested. It is not part of the config being validated.
      await storeDraftSecrets(draft);
      const response = await adminCall<ValidateSetupRequest, ValidateSetupResponse>(
        'validate_setup',
        {
          candidate_config: toSetupConfig(draft)
        }
      );
      setValidation(response.validation);
      setPhase('idle');
    } catch (error) {
      handleError(error);
    }
  };

  const apply = async () => {
    setPhase('applying');
    setNotice(null);
    try {
      await storeDraftSecrets(draft);
      const response = await adminCall<ApplySetupConfigRequest, ApplySetupConfigResponse>(
        'apply_setup_config',
        { config: toSetupConfig(draft) }
      );
      setValidation(response.validation);
      if (response.status === 'ready') {
        await queryClient.invalidateQueries({ queryKey: SETUP_STATE_KEY });
        setPhase('applied');
      } else {
        setPhase('soft_fail');
      }
    } catch (error) {
      handleError(error);
    }
  };

  // Two things, in this order: send the operator to the overview, then close
  // the gate. Closing it is what swaps this full-screen wizard out for the
  // shell — setup owns no route, so nothing else can end it.
  const goToOverview = () => {
    navigate('/');
    onComplete();
  };

  if (phase === 'applied') {
    return (
      <div className={styles.layout}>
        <Banner variant="success" title="You're live">
          Your liquidity provider is configured and published.
        </Banner>

        <section className={styles.section}>
          <h2 className={styles.sectionTitle}>What happens now</h2>

          <div className={styles.liveList}>
            <span>Your advertisement is being published to the network.</span>

            <span>Initiators and guardians can now discover your liquidity.</span>

            <span>Monitor status and funds from the overview.</span>
          </div>
        </section>

        <div>
          <Button variant="primary" onClick={goToOverview}>
            Go to overview
          </Button>
        </div>
      </div>
    );
  }

  const gatewayCredential =
    draft.secrets.gatewayAdminCredential.trim() === '' ? 'credential not set' : 'credential set';
  const attesters = draft.policy.accepted_attester_policies;
  const summaryRows = [
    { label: 'Network', value: draft.network },
    {
      label: 'Gateway',
      value: `${draft.gateway.gateway_name || '—'} · ${draft.gateway.admin_url || '—'} · ${gatewayCredential}`
    },
    {
      label: 'Chain observer',
      value: `${draft.chain_observer.backend.type} · ${draft.chain_observer.backend.url ? truncateMiddle(draft.chain_observer.backend.url, 14, 10) : '—'}`
    },
    {
      label: 'Relays',
      value:
        draft.relays.length > 0
          ? `${draft.relays.length} · ${draft.relays.map((relay) => truncateMiddle(relay, 14, 10)).join(', ')}`
          : 'None'
    },
    {
      label: 'Capacity',
      value:
        draft.capacity.mode === 'explicit_cap'
          ? `Explicit cap · ${draft.capacity.explicit_cap ?? 0} SATS`
          : 'Track available funds'
    },
    {
      label: 'Alerts',
      value: `warn ${draft.replenishment.warning_threshold} · critical ${draft.replenishment.critical_threshold}`
    },
    {
      label: 'Trusted attesters',
      value:
        attesters.length > 0
          ? `${attesters.length} · ${truncateMiddle(attesters[0].attester_pubkey)}`
          : 'None'
    }
  ];

  const failedChecks = validation?.checks.filter((check) => !isPass(check)) ?? [];
  const applyDisabled = !draftComplete(draft) || phase === 'applying' || phase === 'validating';
  return (
    <div className={styles.layout}>
      <section className={styles.section}>
        <h2 className={styles.sectionTitle}>Configuration</h2>

        <div className={styles.summaryList}>
          {summaryRows.map((row) => (
            <div key={row.label} className={styles.summaryRow}>
              <span className={styles.summaryLabel}>{row.label}</span>

              <span className={styles.summaryValue}>{row.value}</span>
            </div>
          ))}
        </div>
      </section>

      {notice ? (
        <Banner
          variant={notice.variant}
          title={notice.variant === 'info' ? 'Not available yet' : "Couldn't apply"}
        >
          {notice.message}
        </Banner>
      ) : null}

      {phase === 'soft_fail' ? (
        <Banner variant="error" title={`Couldn't apply — ${failedChecks.length} checks failed`}>
          Your settings are saved as a draft. You are not published. Fix the checks below and apply
          again.
        </Banner>
      ) : null}

      <section className={styles.section}>
        <h2 className={styles.sectionTitle}>Validation</h2>

        <p className={styles.note}>Validation is a dry run — nothing is saved until you apply.</p>

        <div>
          <Button variant="secondary" onClick={runValidation} loading={phase === 'validating'}>
            Re-run validation
          </Button>
        </div>

        {validation ? (
          <ul className={styles.checkList}>
            {validation.checks.map((check) => (
              <li key={check.name} className={styles.checkRow}>
                <span className={isPass(check) ? styles.passMark : styles.failMark}>
                  {isPass(check) ? '✓' : '✕'}
                </span>

                <div>
                  <span>{check.name}</span>
                  {check.detail ? (
                    <span className={styles.checkDetail}> — {check.detail}</span>
                  ) : null}
                </div>
              </li>
            ))}
          </ul>
        ) : null}
      </section>

      <section className={styles.actions}>
        <Button
          variant="primary"
          onClick={apply}
          disabled={applyDisabled}
          loading={phase === 'applying'}
        >
          Apply & go live
        </Button>

        {!draftComplete(draft) ? (
          <p className={styles.note}>Complete every earlier step before applying.</p>
        ) : null}
      </section>
    </div>
  );
};
