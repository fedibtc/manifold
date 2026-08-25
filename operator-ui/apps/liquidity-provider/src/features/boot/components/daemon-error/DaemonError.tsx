import styles from './DaemonError.module.css';

interface DaemonErrorProps {
  onRetry: () => void;
}

// G1 — daemon unreachable. Full-page retry screen (13-error-daemon.html).
export const DaemonError = ({ onRetry }: DaemonErrorProps) => (
  <div className={styles.page}>
    <div className={styles.card}>
      <div className={styles.glyph} aria-hidden="true">
        !
      </div>

      <h1 className={styles.title}>Can't reach the FLIP daemon</h1>

      <p className={styles.bodyIntro}>
        The dashboard is fine, but the daemon behind it isn't answering. Your funds and
        configuration are untouched — this is a connection problem, not a wallet problem.
      </p>

      <div className={styles.detail}>GET /health · connection refused</div>

      <button className={styles.retry} type="button" onClick={onRetry}>
        Retry
      </button>

      <p className={styles.help}>
        Still failing? Check that the FLIP service is running, then retry. It reconnects
        automatically as soon as the daemon is back.
      </p>
    </div>
  </div>
);
