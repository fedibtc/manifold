import {
  type DaemonFailureKind,
  daemonFailureKind,
  describeDaemonFailure
} from '@/shared/utils/describeDaemonFailure';
import styles from './DaemonError.module.css';

interface DaemonErrorProps {
  failure: unknown;
  onRetry: () => void;
}

// Guardians are supervised child processes, lifetime-coupled to this daemon — they
// do not keep running independently while it is down. Say so plainly (US-FMAN-002);
// never imply seats survive a daemon outage. That is true of an outage and only of
// an outage, so the refused copy makes no claim about the guardians: a daemon that
// answered 403 is running, and its seats with it.
const COPY: Record<DaemonFailureKind, { title: string; intro: string; help: string }> = {
  unreachable: {
    title: "Can't reach the fleet manager",
    intro:
      "The dashboard is fine, but the daemon behind it isn't answering. Your guardians are supervised by this daemon and are down too while it's unreachable — this is a connection problem, not data loss.",
    help: 'Still failing? Check that the fleet-manager service is running, then retry. It reconnects automatically as soon as the daemon is back.'
  },
  refused: {
    title: 'The fleet manager refused this dashboard',
    intro:
      'The daemon is running and answered, but it will not serve this dashboard: it refused the request even though the session is valid. Nothing has been lost, and signing in again will not change the answer.',
    help: 'Ask whoever operates this fleet manager to grant this account access, then retry.'
  }
};

// G1 — the daemon is not serving this dashboard, either because nothing answered
// or because what answered refused.
//
// The detail line states the observed failure. It used to read "connection
// refused" whatever had happened, which told an operator to go looking for a
// dead process when the daemon had in fact answered with a 500.
export const DaemonError = ({ failure, onRetry }: DaemonErrorProps) => {
  const copy = COPY[daemonFailureKind(failure)];
  const detail = describeDaemonFailure(failure);

  return (
    <div className={styles.page}>
      <div className={styles.card}>
        <div className={styles.glyph} aria-hidden="true">
          !
        </div>

        <h1 className={styles.title}>{copy.title}</h1>

        <p className={styles.introText}>{copy.intro}</p>

        <div className={styles.detail}>{detail}</div>

        <button className={styles.retry} type="button" onClick={onRetry}>
          Retry
        </button>

        <p className={styles.help}>{copy.help}</p>
      </div>
    </div>
  );
};
