import { Banner, Button } from '@operator-ui/common-ui';
import { QRCodeSVG } from 'qrcode.react';
import { useEffect } from 'react';
import { useGuardianLink } from '@/features/phone-link/api/hooks/use-guardian-link/useGuardianLink';
import styles from '@/features/phone-link/components/guardian-link/GuardianLink.module.css';
import { useGuardianLinkActions } from '@/features/phone-link/components/guardian-link/useGuardianLinkActions';
import { QuerySurface } from '@/shared/components/query-surface/QuerySurface';
import { useQueryDisposition } from '@/shared/query/use-query-disposition/useQueryDisposition';
import { formatCheckedAt, formatDate } from '@/shared/utils/format';

interface GuardianLinkProps extends Record<string, never> {}

const formatDay = (seconds: number) => formatDate(seconds * 1000);

// What the host does on the phone, in the order the app asks for it: the main
// scan button, the confirm screen, then the system's notification prompt.
const LINK_STEPS: { title: string; detail: string }[] = [
  {
    title: 'Scan this code',
    detail: 'Open the app on your phone, tap Scan/Paste and point the camera here.'
  },
  {
    title: 'Tap Link guardian',
    detail: 'The app shows a confirm screen right after the scan.'
  },
  {
    title: 'Allow notifications',
    detail: 'If your phone asks, allow them. This host uses them to reach you.'
  }
];

const phoneIcon = (
  <svg aria-hidden="true" viewBox="0 0 24 24" className={styles.emptyIcon}>
    <rect x="6" y="2" width="12" height="20" rx="2" />

    <path d="M11 18h2" />
  </svg>
);

export const GuardianLink = (_props: GuardianLinkProps) => {
  const status = useGuardianLink();
  const { disposition, retry } = useQueryDisposition([status]);
  const link = status.data?.link;
  const offer = status.data?.offer;
  const terminal = link?.delivery.state === 'terminal';
  const {
    busy,
    creating,
    createFailed,
    handleCreate,
    handleRevoke,
    handleTest,
    error,
    testMessage
  } = useGuardianLinkActions(link?.linked_at, terminal);
  const lastNotified =
    link?.last_notified_at != null ? formatCheckedAt(link.last_notified_at) : 'Never';
  // A phone links by scanning, so the code is simply there whenever one is
  // needed: no phone yet, or the current phone can no longer be reached. It is
  // minted again after it expires; a failed mint waits for the next poll.
  const wantsOffer = status.data?.available === true && (!link || terminal) && !offer;
  useEffect(() => {
    if (wantsOffer && !creating && !createFailed) handleCreate();
  }, [wantsOffer, creating, createFailed, handleCreate]);

  return (
    <QuerySurface disposition={disposition} onRetry={retry}>
      {status.data &&
        (status.data.available ? (
          <div className={styles.root}>
            {link && (
              <section className={styles.card}>
                <header className={styles.cardHead}>
                  <h2 className={styles.cardTitle}>{link.device_label}</h2>

                  <span className={styles.caption}>Linked phone</span>
                </header>
                {terminal && (
                  <div role="alert">
                    <Banner variant="warn" title="Notifications stopped">
                      Your phone stopped getting notifications from this host. This happens when the
                      app is reinstalled or the link runs out. Link it again with the code below.
                    </Banner>
                  </div>
                )}

                <dl className={styles.kv}>
                  <div className={styles.kvRow}>
                    <dt className={styles.kvLabel}>Linked since</dt>

                    <dd className={styles.kvValue}>{formatDay(link.linked_at)}</dd>
                  </div>

                  <div className={styles.kvRow}>
                    <dt className={styles.kvLabel}>Link renews by</dt>

                    <dd className={styles.kvValue}>{formatDay(link.callback_expires_at)}</dd>
                  </div>

                  <div className={styles.kvRow}>
                    <dt className={styles.kvLabel}>Last notification</dt>

                    <dd className={styles.kvValue}>{lastNotified}</dd>
                  </div>
                </dl>

                <div className={styles.actions}>
                  {!terminal && (
                    <Button onClick={handleTest} disabled={busy}>
                      Send test notification
                    </Button>
                  )}

                  <Button variant="secondary" onClick={handleRevoke} disabled={busy}>
                    Unlink
                  </Button>
                </div>
                {testMessage && (
                  <p role="status" className={styles.outcome}>
                    {testMessage}
                  </p>
                )}
              </section>
            )}
            {offer && (
              <section className={styles.card}>
                <header className={styles.cardHead}>
                  <h2 className={styles.cardTitle}>
                    {terminal ? 'Link your phone again' : 'Link your phone'}
                  </h2>
                </header>

                <div className={styles.split}>
                  <div className={styles.qrBox}>
                    <QRCodeSVG
                      value={offer.uri}
                      size={208}
                      marginSize={2}
                      role="img"
                      aria-label="Guardian link QR code"
                    />
                  </div>

                  <ol className={styles.rows}>
                    {LINK_STEPS.map((step, index) => (
                      <li key={step.title} className={styles.row}>
                        <span className={styles.rowNumber}>{index + 1}</span>

                        <div className={styles.rowText}>
                          <h3 className={styles.stepTitle}>{step.title}</h3>

                          <p className={styles.stepDetail}>{step.detail}</p>
                        </div>
                      </li>
                    ))}
                  </ol>
                </div>

                <p className={styles.footer}>
                  Once linked, your phone appears on this page. This code works once and expires at{' '}
                  {formatCheckedAt(offer.expires_at)}. A new one replaces it then.
                </p>
              </section>
            )}
            {wantsOffer && !createFailed && (
              <section className={styles.card}>
                <div className={styles.empty}>
                  {phoneIcon}

                  <h2 className={styles.emptyTitle}>No phone linked</h2>

                  <p className={styles.emptyText} role="status">
                    Getting a code for your phone…
                  </p>
                </div>
              </section>
            )}
            {error && (
              <p role="alert" className={styles.error}>
                {error}
              </p>
            )}
          </div>
        ) : (
          <section className={styles.card}>
            <div className={styles.empty}>
              {phoneIcon}

              <h2 className={styles.emptyTitle}>Phone notifications are off</h2>

              <p className={styles.emptyText}>
                This host isn't set up to send phone notifications yet. To turn them on, set{' '}
                <code className={styles.code}>FLEET_MANAGER_PUSH_GATEWAY_ORIGIN</code> and restart
                Manifold Fedimint Guardian.
              </p>
            </div>
          </section>
        ))}
    </QuerySurface>
  );
};
