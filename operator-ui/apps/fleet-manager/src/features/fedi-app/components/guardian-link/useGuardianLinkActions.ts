import { useCreateGuardianLinkOffer } from '@/features/fedi-app/api/hooks/use-create-guardian-link-offer/useCreateGuardianLinkOffer';
import { useRevokeGuardianLink } from '@/features/fedi-app/api/hooks/use-revoke-guardian-link/useRevokeGuardianLink';
import { useTestGuardianLinkNotification } from '@/features/fedi-app/api/hooks/use-test-guardian-link-notification/useTestGuardianLinkNotification';
import { describeActionError } from '@/shared/utils/describeActionError';

const TEST_MESSAGES = {
  delivered: 'Test notification delivered.',
  retryable: 'Test notification could not be delivered. Try again.',
  terminal: 'Notifications stopped. Link your phone again.'
};

export const useGuardianLinkActions = (linkedAt: number | undefined, terminal: boolean) => {
  const create = useCreateGuardianLinkOffer();
  const revoke = useRevokeGuardianLink();
  const test = useTestGuardianLinkNotification();
  const busy = create.isPending || revoke.isPending || test.isPending;

  const resetResults = () => {
    create.reset();
    revoke.reset();
    test.reset();
  };
  const handleCreate = () => {
    resetResults();
    create.mutate();
  };
  const handleRevoke = () => {
    // Revocation closes the offer AND forgets the phone, even while relinking.
    if (
      linkedAt !== undefined &&
      !window.confirm('Unlink this phone? It will no longer receive fleet notifications.')
    )
      return;
    resetResults();
    revoke.mutate();
  };
  const handleTest = () => {
    resetResults();
    if (linkedAt !== undefined) test.mutate(linkedAt);
  };

  const error = create.error ?? revoke.error ?? test.error;
  return {
    busy,
    handleCreate,
    handleRevoke,
    handleTest,
    error: error ? describeActionError(error) : null,
    testMessage:
      test.data && test.variables === linkedAt && (!terminal || test.data.outcome === 'terminal')
        ? TEST_MESSAGES[test.data.outcome]
        : null
  };
};
