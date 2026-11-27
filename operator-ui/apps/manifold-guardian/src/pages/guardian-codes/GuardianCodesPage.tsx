import { Banner, Button, SectionCard } from '@operator-ui/common-ui';
import type { SeatGrantEntry } from '@operator-ui/types';
import { useState } from 'react';
import { Link } from 'react-router-dom';
import { useRevokeSeatGrant } from '@/features/guardian-codes/api/hooks/use-revoke-seat-grant/useRevokeSeatGrant';
import { useSeatGrants } from '@/features/guardian-codes/api/hooks/use-seat-grants/useSeatGrants';
import { GuardianCodeCreate } from '@/features/guardian-codes/components/guardian-code-create/GuardianCodeCreate';
import { GuardianCodeList } from '@/features/guardian-codes/components/guardian-code-list/GuardianCodeList';
import {
  GuardianCodeShare,
  isShareable,
  type ShareableGrant
} from '@/features/guardian-codes/components/guardian-code-share/GuardianCodeShare';
import { RevokeGuardianCodeConfirm } from '@/features/guardian-codes/components/revoke-guardian-code-confirm/RevokeGuardianCodeConfirm';
import { useCapacity } from '@/shared/api/hooks/use-capacity/useCapacity';
import { QuerySurface } from '@/shared/components/query-surface/QuerySurface';
import { useQueryDisposition } from '@/shared/query/use-query-disposition/useQueryDisposition';
import { describeActionError } from '@/shared/utils/describeActionError';
import styles from './GuardianCodesPage.module.css';

type View =
  | { kind: 'list' }
  | { kind: 'create' }
  | { kind: 'share'; grant: ShareableGrant; isNew: boolean };

export const GuardianCodesPage = () => {
  const grants = useSeatGrants();
  const capacity = useCapacity();
  const revoke = useRevokeSeatGrant();
  const [view, setView] = useState<View>({ kind: 'list' });
  const [revoking, setRevoking] = useState<SeatGrantEntry | null>(null);
  const { disposition, retry } = useQueryDisposition([grants]);
  const noFreeSeats = capacity.data?.available_slots === 0;

  const handleCreated = (grant: SeatGrantEntry) => {
    if (isShareable(grant)) setView({ kind: 'share', grant, isNew: true });
  };
  const handleShow = (grant: ShareableGrant) => setView({ kind: 'share', grant, isNew: false });
  const handleBack = () => setView({ kind: 'list' });
  const handleRevoke = (grant: SeatGrantEntry) => {
    revoke.reset();
    setRevoking(grant);
  };
  const handleCancelRevoke = () => setRevoking(null);
  const handleConfirmRevoke = () => {
    if (!revoking) return;
    revoke.mutate(revoking.grant_nonce, { onSuccess: () => setRevoking(null) });
  };

  const list = grants.data?.grants ?? [];

  return (
    <div className={styles.root}>
      <div className={styles.head}>
        <h1 className={styles.heading}>Guardian codes</h1>

        <p className={styles.intro}>
          Share a guardian code with a Federation Ambassador you know, and they can add this
          guardian to their federation. Each code works for one federation setup and is always free.
          You don’t need a badge.
        </p>
      </div>

      {noFreeSeats ? (
        <Banner variant="warn" title="No free seats">
          A Federation Ambassador can’t use a guardian code until a seat frees up on this host.
          Codes you create now stay valid.{' '}
          <Link to="/offer" className={styles.inlineLink}>
            Change your seat limit
          </Link>
        </Banner>
      ) : null}

      {view.kind === 'create' ? (
        <GuardianCodeCreate onCreated={handleCreated} onCancel={handleBack} />
      ) : null}

      {view.kind === 'share' ? (
        <GuardianCodeShare grant={view.grant} isNew={view.isNew} onDone={handleBack} />
      ) : null}

      {view.kind === 'list' ? (
        <div className={styles.actions}>
          <Button onClick={() => setView({ kind: 'create' })}>Create a guardian code</Button>
        </div>
      ) : null}

      <QuerySurface disposition={disposition} onRetry={retry}>
        <SectionCard title="Your guardian codes" frame={list.length > 0 ? 'table' : 'card'}>
          {list.length > 0 ? (
            <GuardianCodeList grants={list} onShow={handleShow} onRevoke={handleRevoke} />
          ) : (
            <p className={styles.empty}>
              No guardian codes yet. Create one when a Federation Ambassador you know asks for this
              guardian.
            </p>
          )}
        </SectionCard>

        {list.length > 0 ? (
          <p className={styles.caption}>
            Revoke stops an unused code from being used. A used code has already added this guardian
            to a federation; that seat stays, so a used code can’t be revoked.
          </p>
        ) : null}
      </QuerySurface>

      {revoking ? (
        <RevokeGuardianCodeConfirm
          grant={revoking}
          onConfirm={handleConfirmRevoke}
          onCancel={handleCancelRevoke}
          isPending={revoke.isPending}
          error={revoke.error ? describeActionError(revoke.error) : null}
        />
      ) : null}
    </div>
  );
};
