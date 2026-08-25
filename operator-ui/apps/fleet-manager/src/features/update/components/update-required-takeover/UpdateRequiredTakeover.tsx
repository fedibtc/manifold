import { Button } from '@operator-ui/common-ui';
import { useEffect, useRef } from 'react';
import { useUpdateTakeover } from '@/features/update/components/update-required-takeover/useUpdateTakeover';
import styles from './UpdateRequiredTakeover.module.css';

const HEADING_ID = 'fman-update-heading';

/**
 * The whole viewport, when a newer FMan release has been published.
 *
 * Advisory, not a lockout. `latest` comes off a relay publication, so a
 * blocking screen would let one bad publication shut every operator out of
 * their own host.
 *
 * Mounted by `AppShell`, which is why it can never interrupt sign-in or setup —
 * both sit above the shell in the gate chain. It is not a gate itself: it
 * renders beside the routed outlet rather than instead of it, and covers the
 * screen by being fixed to the viewport.
 *
 * Known gap, for design review: focus is moved here and Escape closes it, but
 * nothing traps Tab inside. There is no dialog primitive in this repository to
 * inherit that from, and inventing one is a design decision rather than an
 * implementation one.
 */
export const UpdateRequiredTakeover = () => {
  const { update, onDismiss } = useUpdateTakeover();
  const dismissButtonRef = useRef<HTMLButtonElement>(null);
  const isOpen = update !== null;

  useEffect(() => {
    if (isOpen) dismissButtonRef.current?.focus();
  }, [isOpen]);

  if (!update) return null;

  return (
    <div className={styles.root} role="dialog" aria-modal="true" aria-labelledby={HEADING_ID}>
      <div className={styles.card}>
        <h1 className={styles.heading} id={HEADING_ID}>
          Update this Fleet Manager
        </h1>

        <p className={styles.intro}>
          A newer release has been published. Update this host through the platform you installed it
          from.
        </p>

        <dl className={styles.versions}>
          <div className={styles.versionRow}>
            <dt className={styles.versionLabel}>Running</dt>

            <dd className={styles.versionValue}>{update.current}</dd>
          </div>

          <div className={styles.versionRow}>
            <dt className={styles.versionLabel}>Latest</dt>

            <dd className={styles.versionValue}>{update.latest}</dd>
          </div>
        </dl>

        <p className={styles.note}>This does not stop your fleet. You can update later.</p>

        <Button ref={dismissButtonRef} fullWidth onClick={onDismiss}>
          Continue to the dashboard
        </Button>
      </div>
    </div>
  );
};
