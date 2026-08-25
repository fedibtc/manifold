import { Banner, Button, CheckboxField } from '@operator-ui/common-ui';
import { useQueryClient } from '@tanstack/react-query';
import { type FormEvent, useState } from 'react';
import { useOnboardFromBackup } from '@/features/setup/api/hooks/use-onboard-from-backup/useOnboardFromBackup';
import { SetupRestoreFailed } from '@/features/setup/components/setup-restore-failed/SetupRestoreFailed';
import { SetupRestoreSuccess } from '@/features/setup/components/setup-restore-success/SetupRestoreSuccess';
import { SetupRestoreUnknown } from '@/features/setup/components/setup-restore-unknown/SetupRestoreUnknown';
import {
  classifyRestoreError,
  type RestoreViewState
} from '@/features/setup/utils/restoreViewState';
import { isNotOnboardedError } from '@/features/setup/utils/setupState';
import { AuthError } from '@/shared/api/errors';
import { fetchOnboarding, ONBOARDING_KEY } from '@/shared/api/hooks/use-onboarding/useOnboarding';
import styles from './SetupRestore.module.css';

interface SetupRestoreProps {
  onRestored: () => void;
  onCancel: () => void;
}

export const SetupRestore = ({ onRestored, onCancel }: SetupRestoreProps) => {
  const restore = useOnboardFromBackup();
  const queryClient = useQueryClient();
  const [mnemonic, setMnemonic] = useState('');
  const [acknowledged, setAcknowledged] = useState(false);
  // The screen is selected from here, never from the mutation. The mutation is
  // reset as soon as its result has been copied across, and an idle mutation can
  // select nothing.
  const [view, setView] = useState<RestoreViewState>({ type: 'form' });
  const [isChecking, setIsChecking] = useState(false);
  const [identityConfirmed, setIdentityConfirmed] = useState(false);

  const handleMnemonicChange = (event: FormEvent<HTMLTextAreaElement>) => {
    setMnemonic(event.currentTarget.value);
  };

  const handleSubmit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    restore.mutate(
      { mnemonic: mnemonic.trim(), acknowledgeOriginalHostIsGone: acknowledged },
      {
        onSuccess: (response) => {
          setView({ type: 'success', result: { seats: response.seats, formed: response.formed } });
          // There is no way back to the form from here, so the field's copy has no
          // further use. `reset()` drops the mutation's copy.
          setMnemonic('');
          restore.reset();
        },
        onError: async (error) => {
          const safe = classifyRestoreError(error);
          restore.reset();
          // The acknowledgement re-arms on every failed attempt. It is the gate
          // against two hosts running one guardian identity — the failure this
          // screen warns no check can catch — so the operator asserts it per
          // attempt, not once per visit.
          setAcknowledged(false);

          if (safe.errorClass === 'auth') {
            // The sign-in gate reads authentication errors off the Onboarding query
            // only, so a mutation error cannot open it. An awaited refetch — not an
            // invalidation — makes the gate open now instead of at the next poll.
            setMnemonic('');
            setView({ type: 'form' });
            await queryClient.refetchQueries({ queryKey: ONBOARDING_KEY, exact: true });
            return;
          }

          if (safe.errorClass === 'daemon') {
            // The one branch that keeps the phrase, and only until the operator
            // acts on the refusal. The daemon answered and installed nothing, and
            // most of what it refuses with is a fault outside the phrase — a seat
            // directory left by an earlier attempt, an archive not yet on the
            // relays — which the operator clears and then retries with the very
            // same twelve words. Erasing them here does not protect the phrase so
            // much as relocate it: an operator retyping twelve words per attempt
            // reaches for a photograph or a note file to paste from, which is the
            // one thing this flow tells them never to keep.
            //
            // Bounded, and by what: "Back to setup options" clears it, and
            // unmounting the wizard takes it with the component. "Try again" is
            // deliberately not a bound — that is the retry this serves — but it
            // returns to a form whose acknowledgement has just been cleared, so
            // the operator cannot resubmit without asserting the dangerous claim
            // again. The unknown branch gets none of this: it has no bound.
            setView({ type: 'failed', error: safe });
            return;
          }

          // Unknown: the daemon may have installed the identity before the
          // response was lost, and its screen waits indefinitely for an explicit
          // status check. Indefinite is the word that decides this — no bound, so
          // no retention.
          setMnemonic('');
          setView({ type: 'unknown', error: safe });
        }
      }
    );
  };

  const handleTryAgain = () => {
    setView({ type: 'form' });
  };

  const handleBackToDoors = () => {
    setMnemonic('');
    onCancel();
  };

  // The daemon may have installed the identity before the response was lost, so the
  // only safe next move is to ask it what it now is.
  //
  // The call is made directly rather than through `fetchQuery` or a refetch, both
  // of which answer with whatever request is already in flight for this key —
  // during setup that is a poll issued before the restore, so the check would
  // report a reading from before the event it is checking.
  const handleCheckStatus = async () => {
    setIsChecking(true);
    try {
      queryClient.setQueryData(ONBOARDING_KEY, await fetchOnboarding());
      setIdentityConfirmed(true);
    } catch (error) {
      // A daemon that says it was never onboarded has settled the question: the
      // restore did not land, so the form is where the operator belongs.
      if (isNotOnboardedError(error)) {
        setView({ type: 'form' });
      } else if (error instanceof AuthError) {
        // A direct call bypasses the query cache, so nothing else can raise the
        // sign-in gate from here — the gate reads the Onboarding query alone.
        await queryClient.refetchQueries({ queryKey: ONBOARDING_KEY, exact: true });
      }
      // Anything else leaves the screen where it is — an unreachable daemon is the
      // boot gate's problem, and it already owns that screen.
    } finally {
      setIsChecking(false);
    }
  };

  const canSubmit = mnemonic.trim().length > 0 && acknowledged && !restore.isPending;

  if (view.type === 'success') {
    return <SetupRestoreSuccess result={view.result} onContinue={onRestored} />;
  }

  if (view.type === 'failed') {
    return (
      <SetupRestoreFailed
        error={view.error}
        onTryAgain={handleTryAgain}
        onBackToDoors={handleBackToDoors}
      />
    );
  }

  if (view.type === 'unknown') {
    return (
      <SetupRestoreUnknown
        error={view.error}
        isChecking={isChecking}
        identityConfirmed={identityConfirmed}
        onCheckStatus={handleCheckStatus}
        onContinue={onRestored}
      />
    );
  }

  return (
    <div className={styles.root}>
      <h1 className={styles.heading}>Recover from your phrase</h1>

      <Banner variant="error">
        The guardians this phrase belongs to have been transferred to this host. Only continue if
        the original host is permanently offline — two hosts running one guardian identity will
        equivocate, and no check here can catch it.
      </Banner>

      <form className={styles.form} onSubmit={handleSubmit}>
        <label className={styles.label} htmlFor="recovery-phrase">
          Recovery phrase
        </label>

        <textarea
          id="recovery-phrase"
          className={styles.textarea}
          rows={3}
          value={mnemonic}
          autoComplete="off"
          autoCapitalize="none"
          autoCorrect="off"
          spellCheck={false}
          onChange={handleMnemonicChange}
        />

        <CheckboxField
          label="I confirm the original host and its guardians are permanently offline"
          checked={acknowledged}
          onChange={setAcknowledged}
        />

        <div className={styles.actions}>
          {/* Leaving mid-flight unmounts this screen, which drops the mutation's
              observer: the restore would still land, with nothing left to report
              it. The operator waits out the request they started. */}
          <Button variant="secondary" disabled={restore.isPending} onClick={handleBackToDoors}>
            Back
          </Button>

          <Button type="submit" disabled={!canSubmit} loading={restore.isPending}>
            Recover this fleet
          </Button>
        </div>
      </form>
    </div>
  );
};
