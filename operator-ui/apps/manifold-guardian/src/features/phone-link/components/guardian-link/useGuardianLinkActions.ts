import { useCallback } from 'react';
import { useCreateGuardianLinkOffer } from '@/features/phone-link/api/hooks/use-create-guardian-link-offer/useCreateGuardianLinkOffer';
import { useRevokeGuardianLink } from '@/features/phone-link/api/hooks/use-revoke-guardian-link/useRevokeGuardianLink';
import { useTestGuardianLinkNotification } from '@/features/phone-link/api/hooks/use-test-guardian-link-notification/useTestGuardianLinkNotification';
import { describeActionError } from '@/shared/utils/describeActionError';

const TEST_MESSAGES = {
  delivered: 'Test alert sent. Check your phone.',
  retryable: 'Test alert could not be sent. Try again.',
  inactive: 'Test alert did not reach your phone. Open the Fedi app on it.',
  rateLimited: "You've used this hour's test alerts. Try again later.",
  terminal: 'Notifications stopped. Reconnect your phone.'
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
  const handleCreate = useCallback(() => {
    revoke.reset();
    test.reset();
    create.mutate();
  }, [create.mutate, revoke.reset, test.reset]);
  const handleRevoke = () => {
    // Revocation closes the offer AND forgets the phone, even while relinking.
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
    creating: create.isPending,
    createFailed: create.isError,
    handleCreate,
    handleRevoke,
    handleTest,
    error: error ? describeActionError(error) : null,
    testMessage:
      test.data && test.variables === linkedAt && (!terminal || test.data.outcome === 'terminal')
        ? test.data.reason === 'target_inactive'
          ? TEST_MESSAGES.inactive
          : test.data.reason === 'rate_limited'
            ? TEST_MESSAGES.rateLimited
            : TEST_MESSAGES[test.data.outcome]
        : null
  };
};
