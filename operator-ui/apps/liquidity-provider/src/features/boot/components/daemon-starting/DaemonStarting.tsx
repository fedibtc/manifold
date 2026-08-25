import styles from './DaemonStarting.module.css';

export type StartingReason = 'reloading' | 'no-runtime';

const COPY: Record<StartingReason, { title: string; intro: string; detail: string }> = {
  reloading: {
    title: 'Restoring from a backup',
    intro:
      'The daemon is rebuilding against the restored state. Your funds and configuration are untouched — nothing is lost while this runs.',
    detail: 'GET /health · mode: reloading'
  },
  'no-runtime': {
    title: 'The FLIP daemon is starting',
    intro:
      'The daemon is answering, but has not finished building its runtime yet. This is a normal start, not a fault.',
    detail: 'GET /health · mode: no_runtime'
  }
};

interface DaemonStartingProps {
  reason: StartingReason;
  onRetry: () => void;
}

/**
 * The daemon is up and has told us what it is doing, but cannot serve the Admin
 * API yet.
 *
 * Both states used to land on the daemon-unreachable screen, which says the
 * daemon "isn't answering" over the line `GET /health · connection refused`.
 * That route is precisely what *is* answering — it returns 200 and names the
 * mode — so the screen stated something the operator could disprove in seconds,
 * and it did so during a restore, which is the worst moment to send someone
 * hunting for a network fault.
 *
 * No controls. Nothing an operator can do shortens either wait, and every
 * runtime-backed route refuses until the runtime exists. The retry is here
 * because a person watching a slow restore will look for a button, not because
 * waiting needs one.
 */
export const DaemonStarting = ({ reason, onRetry }: DaemonStartingProps) => {
  const copy = COPY[reason];

  return (
    <div className={styles.page}>
      <div className={styles.card}>
        <h1 className={styles.title}>{copy.title}</h1>

        <p className={styles.bodyIntro}>{copy.intro}</p>

        <div className={styles.detail}>{copy.detail}</div>

        <button className={styles.retry} type="button" onClick={onRetry}>
          Check now
        </button>

        <p className={styles.help}>This screen clears by itself as soon as the daemon finishes.</p>
      </div>
    </div>
  );
};
