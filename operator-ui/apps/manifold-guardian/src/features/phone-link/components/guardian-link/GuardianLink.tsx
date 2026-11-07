import { Banner, Button, Chip } from '@operator-ui/common-ui';
import { QRCodeSVG } from 'qrcode.react';
import { type ReactNode, useEffect, useRef, useState } from 'react';
import { useGuardianLink } from '@/features/phone-link/api/hooks/use-guardian-link/useGuardianLink';
import styles from '@/features/phone-link/components/guardian-link/GuardianLink.module.css';
import { useGuardianLinkActions } from '@/features/phone-link/components/guardian-link/useGuardianLinkActions';
import { QuerySurface } from '@/shared/components/query-surface/QuerySurface';
import { useQueryDisposition } from '@/shared/query/use-query-disposition/useQueryDisposition';
import { formatLocalDay, formatLocalMoment } from '@/shared/utils/format';

interface GuardianLinkProps extends Record<string, never> {}

// What the host does on the phone, in the order the app asks for it: the main
// scan button, the confirm screen, then the system's notification prompt.
const LINK_STEPS: { title: string; detail: string }[] = [
  {
    title: 'Scan this code',
    detail: 'Open the Fedi app on your phone, tap Scan/Paste and point the camera here.'
  },
  {
    title: 'Tap Link guardian',
    detail: 'The app opens “Link your guardian” right after the scan.'
  },
  {
    title: 'Allow notifications',
    detail: 'If your phone asks, allow them. This is how your guardian reaches you.'
  }
];

const icon = (paths: ReactNode) => (
  <svg aria-hidden="true" viewBox="0 0 24 24" className={styles.icon}>
    {paths}
  </svg>
);

const phoneIcon = icon(
  <>
    <rect x="6" y="2" width="12" height="20" rx="2" />

    <path d="M11 18h2" />
  </>
);

// What the daemon notifies about (`AttentionReason` in mg-core), in words.
const NOTIFIED_ABOUT: { text: string; icon: ReactNode }[] = [
  { text: 'A Health check fails', icon: icon(<path d="M3 12h4l3-7 4 14 3-7h4" />) },
  { text: 'An update is recommended', icon: icon(<path d="M12 3v12m-5-5 5 5 5-5M5 21h14" />) },
  {
    text: 'Support writes to you',
    icon: icon(<path d="M4 5h16v12H9l-5 4z" />)
  }
];

const bellIcon = icon(<path d="M6 16V11a6 6 0 0 1 12 0v5l2 2H4zM10 21h4" />);

const notifiedAbout = (
  <section className={styles.card}>
    <h2 className={styles.cardTitle}>What you’ll get notified about</h2>

    <ul className={styles.notified} aria-label="What you’ll get notified about">
      {NOTIFIED_ABOUT.map((item) => (
        <li key={item.text} className={styles.notifiedRow}>
          {item.icon}
          {item.text}
        </li>
      ))}
    </ul>

    <p className={styles.note}>
      You get one alert when a problem starts, and no reminders after that.
    </p>
  </section>
);

// The status poll is the only signal that a scan worked, so a link landing
// while the page is open is told apart from one that was already there.
const useJustLinked = (loaded: boolean, linkedAt: number | undefined) => {
  const seen = useRef<number | undefined>(undefined);
  const wasLoaded = useRef(false);
  const [justLinked, setJustLinked] = useState<number | null>(null);
  useEffect(() => {
    if (!loaded) return;
    if (wasLoaded.current && linkedAt !== undefined && linkedAt !== seen.current) {
      setJustLinked(linkedAt);
    }
    wasLoaded.current = true;
    seen.current = linkedAt;
  }, [loaded, linkedAt]);
  return {
    justLinked: justLinked !== null && justLinked === linkedAt,
    dismiss: () => setJustLinked(null)
  };
};

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
  const { justLinked, dismiss } = useJustLinked(status.data !== undefined, link?.linked_at);
  // Bound to the link it was asked about: a new phone never inherits the question.
  const [unlinkAsked, setUnlinkAsked] = useState<number | null>(null);
  // With no phone the code is simply there. A stopped phone gets one when the
  // host presses Reconnect. It is minted again after it expires; a failed mint
  // waits for the next poll.
  const wantsOffer = status.data?.available === true && !link && !offer;
  useEffect(() => {
    if (wantsOffer && !creating && !createFailed) handleCreate();
  }, [wantsOffer, creating, createFailed, handleCreate]);

  return (
    <QuerySurface disposition={disposition} onRetry={retry}>
      {status.data &&
        (status.data.available ? (
          <div className={styles.root}>
            {link && justLinked && (
              <div role="status">
                <Banner
                  variant="success"
                  title="Phone connected"
                  action={
                    <button
                      type="button"
                      className={styles.dismiss}
                      aria-label="Dismiss"
                      onClick={dismiss}
                    >
                      ×
                    </button>
                  }
                >
                  {link.device_label} will now get guardian alerts.
                </Banner>
              </div>
            )}
            {link && (
              <section className={terminal ? styles.cardStopped : styles.card}>
                <header className={styles.phoneHead}>
                  {phoneIcon}

                  <div>
                    <h2 className={styles.strongTitle}>{link.device_label}</h2>

                    <p className={styles.note}>Fedi app</p>
                  </div>
                  {terminal ? (
                    <Chip tone="warn">Notifications stopped</Chip>
                  ) : (
                    <Chip tone="ok">Notifications active</Chip>
                  )}
                </header>
                {terminal && (
                  <div role="alert">
                    <Banner variant="warn" title="Your phone stopped getting alerts">
                      The Fedi app was reinstalled or linked to another guardian.
                    </Banner>
                  </div>
                )}

                <dl className={styles.kv}>
                  <div className={styles.kvRow}>
                    <dt className={styles.kvLabel}>Linked since</dt>

                    <dd className={styles.kvValue}>{formatLocalDay(link.linked_at)}</dd>
                  </div>

                  <div className={styles.kvRow}>
                    <dt className={styles.kvLabel}>Last notification</dt>

                    <dd className={styles.kvValue}>
                      {link.last_notified_at != null
                        ? formatLocalMoment(link.last_notified_at)
                        : 'None yet'}
                    </dd>
                  </div>
                </dl>
                {terminal && !offer && (
                  <div className={styles.actions}>
                    <Button onClick={handleCreate} disabled={busy}>
                      Reconnect phone
                    </Button>
                  </div>
                )}
                {!terminal && unlinkAsked !== link.linked_at && (
                  <div className={styles.actions}>
                    <Button variant="secondary" onClick={handleTest} disabled={busy}>
                      <span className={styles.buttonIcon}>
                        {bellIcon}
                        Send a test alert
                      </span>
                    </Button>

                    <Button
                      variant="secondary"
                      onClick={() => setUnlinkAsked(link.linked_at)}
                      disabled={busy}
                    >
                      Unlink
                    </Button>
                  </div>
                )}
                {!terminal && unlinkAsked === link.linked_at && (
                  <div className={styles.confirm}>
                    <p className={styles.confirmText}>
                      Unlink {link.device_label}? It stops getting alerts from this guardian.
                    </p>

                    <div className={styles.actions}>
                      <Button
                        variant="secondary"
                        onClick={() => setUnlinkAsked(null)}
                        disabled={busy}
                      >
                        Cancel
                      </Button>

                      <Button variant="danger" onClick={handleRevoke} disabled={busy}>
                        Unlink
                      </Button>
                    </div>
                  </div>
                )}
                {testMessage && unlinkAsked !== link.linked_at && (
                  <p role="status" className={styles.note}>
                    {testMessage}
                  </p>
                )}
              </section>
            )}
            {offer && (
              <section className={styles.card}>
                <h2 className={styles.cardTitle}>
                  {terminal ? 'Reconnect your phone' : 'Link your phone'}
                </h2>

                <div className={styles.split}>
                  <div className={styles.qrBox}>
                    <QRCodeSVG
                      value={offer.uri}
                      size={168}
                      marginSize={1}
                      role="img"
                      aria-label="Guardian link QR code"
                    />
                  </div>

                  <ol className={styles.rows} aria-label="Link steps">
                    {LINK_STEPS.map((step, index) => (
                      <li key={step.title} className={styles.row}>
                        <span className={styles.stepNumber} aria-hidden="true">
                          {index + 1}
                        </span>

                        <div className={styles.stepBody}>
                          <h3 className={styles.stepTitle}>{step.title}</h3>

                          <p className={styles.note}>{step.detail}</p>
                        </div>
                      </li>
                    ))}
                  </ol>
                </div>

                {link && (
                  <p className={styles.note}>
                    {terminal
                      ? 'Scanning it replaces this phone and the warning goes away.'
                      : 'Scanning it replaces this phone.'}
                  </p>
                )}
              </section>
            )}
            {wantsOffer && !createFailed && (
              <section className={styles.card}>
                <div className={styles.empty}>
                  {phoneIcon}

                  <h2 className={styles.strongTitle}>No phone linked</h2>

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
            {notifiedAbout}
          </div>
        ) : (
          <section className={styles.card}>
            <div className={styles.empty}>
              {phoneIcon}

              <h2 className={styles.strongTitle}>Phone alerts are off on this host</h2>

              <p className={styles.emptyText}>
                Set <code className={styles.code}>FLEET_MANAGER_PUSH_GATEWAY_ORIGIN</code> and
                restart Manifold Fedimint Guardian.
              </p>
            </div>
          </section>
        ))}
    </QuerySurface>
  );
};
