import { Button } from '@operator-ui/common-ui';
import type { HolderPairingInvite, OnboardingResponse } from '@operator-ui/types';
import { useEffect, useState } from 'react';
import { adminCall } from '@/shared/api/adminCall';
import { useOnboarding } from '@/shared/api/hooks/use-onboarding/useOnboarding';
import { AuthorizationQrCode } from '@/shared/components/authorization-qr-code/AuthorizationQrCode';
import { ServiceKeyRow } from '@/shared/components/service-key-row/ServiceKeyRow';
import styles from './HolderPairing.module.css';

const QR_SIZE = 192;

const activeHolder = (data?: OnboardingResponse) =>
  data?.nostr.state === 'authorization_observed' ? data.nostr.holders[0] : undefined;

/** Keep the bearer in mounted component state only: never a query cache,
 *  URL, clipboard, external QR renderer or dashboard status response. */
export const HolderPairing = () => {
  const onboarding = useOnboarding();
  const [invite, setInvite] = useState<HolderPairingInvite>();
  const [previousHolder, setPreviousHolder] = useState<string>();
  const [pending, setPending] = useState(false);
  const [failed, setFailed] = useState(false);
  const [expired, setExpired] = useState(false);
  const [check, setCheck] = useState<'error' | 'waiting' | 'unchanged' | 'approved'>();
  useEffect(() => {
    if (!invite) return;
    const timeout = setTimeout(
      () => {
        setInvite(undefined);
        setExpired(true);
      },
      Math.max(0, invite.expires_at_ms - Date.now())
    );
    return () => clearTimeout(timeout);
  }, [invite]);
  const create = async () => {
    setInvite(undefined);
    setFailed(false);
    setExpired(false);
    setCheck(undefined);
    setPreviousHolder(activeHolder(onboarding.data));
    setPending(true);
    try {
      setInvite(await adminCall<HolderPairingInvite>('CreateHolderPairing'));
    } catch {
      setFailed(true);
    } finally {
      setPending(false);
    }
  };
  const checkApproval = async () => {
    setCheck(undefined);
    const result = await onboarding.refetch();
    if (result.isError) {
      setCheck('error');
      return;
    }
    const holder = activeHolder(result.data);
    if (!holder) {
      setCheck('waiting');
    } else if (holder === previousHolder) {
      setCheck('unchanged');
    } else {
      setInvite(undefined);
      setExpired(false);
      setCheck('approved');
    }
  };

  return (
    <div className={styles.controls}>
      <p className={styles.hint}>
        Use a version of PeerBadge that supports private pairing. The code is single-use and expires
        after five minutes. Only show it to the intended holder.
      </p>

      {invite ? (
        <>
          <AuthorizationQrCode request={JSON.stringify(invite)} size={QR_SIZE} />

          <div className={styles.controls}>
            <span className={styles.hint}>Manifold Fedimint Guardian ID</span>

            <ServiceKeyRow pubkey={invite.subject_pubkey} />
          </div>
        </>
      ) : null}

      <Button onClick={create} loading={pending} disabled={onboarding.isFetching}>
        {invite ? 'Replace pairing code' : 'Create private pairing code'}
      </Button>
      {invite || expired ? (
        <>
          <p role="status" className={styles.hint}>
            {expired
              ? 'The code expired. If you did not finish pairing, create a new code.'
              : 'After PeerBadge says “Paired with guardian,” check status here. Replacing the code invalidates this one.'}
          </p>

          <Button variant="secondary" loading={onboarding.isFetching} onClick={checkApproval}>
            Check status
          </Button>
        </>
      ) : null}

      {check === 'error' ? <p role="alert">Could not check this host. Try again.</p> : null}
      {check === 'waiting' ? (
        <p role="status">This host does not have an active approval yet.</p>
      ) : null}
      {check === 'unchanged' ? (
        <p role="status">This host still shows the same holder under Approved by.</p>
      ) : null}
      {check === 'approved' ? (
        <p role="status">
          This host reports an active approval. Confirm the holder under Approved by.
        </p>
      ) : null}
      {failed ? <p role="alert">Could not create a pairing code. Try again.</p> : null}
    </div>
  );
};
