import { Banner, CopyButton, isTruncated, truncateMiddle } from '@operator-ui/common-ui';
import { Link } from 'react-router-dom';
import { useOnboarding } from '@/shared/api/hooks/use-onboarding/useOnboarding';
import { QuerySurface } from '@/shared/components/query-surface/QuerySurface';
import { useQueryDisposition } from '@/shared/query/use-query-disposition/useQueryDisposition';
import styles from './BackupPage.module.css';

export const BackupPage = () => {
  const onboarding = useOnboarding();

  // The keys below are the operator's own identity, and a failed refresh used to
  // delete them for the whole outage. They now stay on screen under a staleness
  // marker: a key that was correct a minute ago is more use than a blank page,
  // and the marker is what stops it being read as current. Holding the whole
  // block behind one answer is also what removes the per-field "—" fallbacks,
  // which said "unknown" in the one case the daemon had actually answered.
  const { disposition, retry } = useQueryDisposition([onboarding]);
  const identity = onboarding.data;

  const backup = identity ? (
    <>
      <p className={styles.intro}>
        The 12-word recovery phrase is the whole backup. Every key below is derived (HKDF) from it,
        and this fleet's seat records are published to the relay it advertises on — so a recovery
        needs the phrase and nothing else.
      </p>

      <Banner variant="warn">
        Recovery happens only while setting up a host. There is no restore action here: a running
        fleet is already set up, and two hosts sharing one guardian identity would equivocate.
      </Banner>

      <dl className={styles.kv}>
        {/* Ported from the Identity page this one replaced: the derived
            two-word name is how an operator recognises their own FMan, so it
            belongs beside the keys it is derived from. */}
        <div className={styles.kvRow}>
          <dt className={styles.kvLabel}>FMan name</dt>

          <dd className={styles.nameValue}>{identity.fman_name}</dd>
        </div>

        <div className={styles.kvRow}>
          <dt className={styles.kvLabel}>Service pubkey</dt>

          <dd className={styles.idRow}>
            <span className={styles.kvValue}>
              {truncateMiddle(identity.service_pubkey, 10, 10)}
            </span>

            {isTruncated(identity.service_pubkey, 10, 10) && (
              <CopyButton value={identity.service_pubkey} label="Copy service pubkey" />
            )}
          </dd>
        </div>

        <div className={styles.kvRow}>
          <dt className={styles.kvLabel}>Service Nostr pubkey</dt>

          <dd className={styles.idRow}>
            <span className={styles.kvValue}>
              {truncateMiddle(identity.service_nostr_pubkey, 10, 10)}
            </span>

            {isTruncated(identity.service_nostr_pubkey, 10, 10) && (
              <CopyButton value={identity.service_nostr_pubkey} label="Copy service Nostr pubkey" />
            )}
          </dd>
        </div>
      </dl>

      {/* The wizard's step lives in memory only, so a reload during setup loses
          it. The phrase is still reachable from here, and this line says so
          without claiming the operator ever wrote it down. BE-FMAN-SETUP-002
          owns the state that would let the UI know. */}
      <p className={styles.reloadNote}>
        This dashboard did not save your recovery phrase, and it cannot tell whether you wrote it
        down. If setup was interrupted, reveal it here and record it now.
      </p>

      <Link to="/backup/phrase" className={styles.phraseLink}>
        Reveal recovery phrase
      </Link>
    </>
  ) : null;

  return (
    <div className={styles.root}>
      <h1 className={styles.heading}>Backup</h1>

      <QuerySurface disposition={disposition} onRetry={retry}>
        {backup}
      </QuerySurface>
    </div>
  );
};
