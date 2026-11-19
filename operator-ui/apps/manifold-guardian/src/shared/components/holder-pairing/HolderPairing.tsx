import { Button } from '@operator-ui/common-ui';
import type { HolderPairingInvite } from '@operator-ui/types';
import { useEffect, useState } from 'react';
import { adminCall } from '@/shared/api/adminCall';
import { AuthorizationQrCode } from '@/shared/components/authorization-qr-code/AuthorizationQrCode';

/** Keep the bearer in mounted component state only: never a query cache,
 *  URL, clipboard, external QR renderer or dashboard status response. */
export const HolderPairing = () => {
  const [invite, setInvite] = useState<HolderPairingInvite>();
  const [pending, setPending] = useState(false);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    if (!invite) return;
    const timeout = setTimeout(
      () => setInvite(undefined),
      Math.max(0, invite.expires_at_ms - Date.now())
    );
    return () => clearTimeout(timeout);
  }, [invite]);
  const create = async () => {
    setInvite(undefined);
    setFailed(false);
    setPending(true);
    try {
      setInvite(await adminCall<HolderPairingInvite>('CreateHolderPairing'));
    } catch {
      setFailed(true);
    } finally {
      setPending(false);
    }
  };
  return (
    <section>
      <p>
        Scan privately with a PeerBadge app that supports direct holder pairing. Whoever scans can
        set this host’s holder. A new code invalidates the previous one.
      </p>

      <Button onClick={create} loading={pending}>
        {invite ? 'Replace pairing code' : 'Create private pairing code'}
      </Button>
      {invite ? <AuthorizationQrCode request={JSON.stringify(invite)} size={256} /> : null}
      {failed ? <p role="alert">Could not create a pairing code. Try again.</p> : null}
    </section>
  );
};
