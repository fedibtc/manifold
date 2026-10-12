import { Button, SectionCard } from '@operator-ui/common-ui';
import { QRCodeSVG } from 'qrcode.react';
import { useGuardianLink } from '@/features/fedi-app/api/hooks/use-guardian-link/useGuardianLink';
import styles from '@/features/fedi-app/components/guardian-link/GuardianLink.module.css';
import { useGuardianLinkActions } from '@/features/fedi-app/components/guardian-link/useGuardianLinkActions';
import { QuerySurface } from '@/shared/components/query-surface/QuerySurface';
import { useQueryDisposition } from '@/shared/query/use-query-disposition/useQueryDisposition';

interface GuardianLinkProps extends Record<string, never> {}

const formatDate = (seconds: number) => new Date(seconds * 1000).toLocaleString();

export const GuardianLink = (_props: GuardianLinkProps) => {
  const status = useGuardianLink();
  const { disposition, retry } = useQueryDisposition([status]);
  const link = status.data?.link;
  const offer = status.data?.offer;
  const terminal = link?.delivery.state === 'terminal';
  const { busy, handleCreate, handleRevoke, handleTest, error, testMessage } =
    useGuardianLinkActions(link?.linked_at, terminal);
  const linkedSince = link ? formatDate(link.linked_at) : '';
  const renewalDate = link ? formatDate(link.callback_expires_at) : '';
  const lastNotified = link?.last_notified_at != null ? formatDate(link.last_notified_at) : 'Never';
  const expiresAt = offer ? formatDate(offer.expires_at) : '';

  return (
    <QuerySurface disposition={disposition} onRetry={retry}>
      {status.data && (
        <SectionCard title="Fleet notifications">
          <div className={styles.root}>
            {status.data.available ? (
              <>
                <p className={styles.intro}>
                  Link your Fedi app to this Fleet Manager to receive a notification when your fleet
                  needs attention or Fedi support writes to you.
                </p>

                {link && (
                  <div className={styles.link}>
                    <h3 className={styles.title}>{link.device_label}</h3>
                    {terminal && (
                      <p role="alert" className={styles.warning}>
                        Notifications stopped. The push gateway rejected this phone's notification
                        hook. Link your phone again to resume notifications.
                      </p>
                    )}
                    <dl className={styles.details}>
                      <dt>Linked since</dt>

                      <dd>{linkedSince}</dd>

                      <dt>Hook renewal date</dt>

                      <dd>{renewalDate}</dd>

                      <dt>Last notified</dt>

                      <dd>{lastNotified}</dd>
                    </dl>

                    <div className={styles.actions}>
                      {terminal ? (
                        <Button onClick={handleCreate} disabled={busy}>
                          Link again
                        </Button>
                      ) : (
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
                  </div>
                )}

                {offer && (
                  <div className={styles.offer}>
                    <h3 className={styles.title}>Scan to link your phone</h3>

                    <p className={styles.intro}>
                      Open the Fedi app, go to Guardian link, scan this code.
                    </p>

                    <QRCodeSVG
                      className={styles.qr}
                      value={offer.uri}
                      marginSize={4}
                      role="img"
                      aria-label="Guardian link QR code"
                    />

                    <p className={styles.intro}>
                      Expires at {expiresAt}. This code is valid for ten minutes.
                    </p>
                    {link && (
                      <p className={styles.intro}>Canceling also unlinks the current phone.</p>
                    )}
                    <div className={styles.actions}>
                      <Button variant="secondary" onClick={handleRevoke} disabled={busy}>
                        Cancel
                      </Button>
                    </div>
                  </div>
                )}

                {!link && !offer && (
                  <div className={styles.actions}>
                    <Button onClick={handleCreate} disabled={busy}>
                      Link a phone
                    </Button>
                  </div>
                )}
                {error && (
                  <p role="alert" className={styles.error}>
                    {error}
                  </p>
                )}
              </>
            ) : (
              <p className={styles.intro}>
                This host has no push gateway configured, so a phone cannot be linked. Ask your host
                administrator to configure a push gateway to enable Fedi app notifications.
              </p>
            )}
          </div>
        </SectionCard>
      )}
    </QuerySurface>
  );
};
