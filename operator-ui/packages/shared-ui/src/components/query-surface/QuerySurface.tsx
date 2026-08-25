import type { ReactNode } from 'react';
import type { QueryDisposition } from '../../query/use-query-disposition/useQueryDisposition';
import { Banner } from '../banner/Banner';
import { Button } from '../button/Button';
import { StaleDataBanner } from '../stale-data-banner/StaleDataBanner';
import styles from './QuerySurface.module.css';

interface QuerySurfaceProps {
  disposition: QueryDisposition;
  onRetry: () => void;
  /**
   * Turns a thrown read error into operator-facing copy. Injected because each
   * dashboard has its own error taxonomy — FLIP's admin API carries service
   * codes, the fleet manager's does not — and the sentence an operator reads
   * has to come from the vocabulary of the daemon they are talking to. Each app
   * binds its own in a one-line wrapper rather than passing this per screen.
   */
  describeError: (error: unknown) => string;
  children: ReactNode;
}

/**
 * The one rendering of the four dispositions. A screen wraps whatever it says
 * about the daemon in this, so an outage teaches the operator the same lesson
 * everywhere: nothing is claimed before the daemon answers, a failure offers a
 * retry, and a held answer stays on screen under a staleness marker rather than
 * being deleted.
 */
export const QuerySurface = ({
  disposition,
  onRetry,
  describeError,
  children
}: QuerySurfaceProps) => {
  if (disposition.kind === 'loading') return <p className={styles.loading}>Loading…</p>;

  if (disposition.kind === 'failed') {
    const retryControl = (
      <Button variant="secondary" size="small" onClick={onRetry}>
        Try again
      </Button>
    );

    return (
      <Banner variant="error" action={retryControl}>
        {describeError(disposition.error)}
      </Banner>
    );
  }

  return (
    <div className={styles.root}>
      {disposition.kind === 'stale' && <StaleDataBanner updatedAtMs={disposition.updatedAtMs} />}

      {children}
    </div>
  );
};
