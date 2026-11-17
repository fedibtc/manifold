import { useState } from 'react';
import { useResetUptime } from '@/shared/api/hooks/use-reset-uptime/useResetUptime';
import { describeActionError } from '@/shared/utils/describeActionError';
import { ResetUptimeConfirm } from './ResetUptimeConfirm';
import styles from './UptimeTitle.module.css';

/** Taps on the title that open the uptime reset. A badge issuer tells the
 *  operator the count; nobody resets by accident. */
export const RESET_UPTIME_TAPS = 21;

/**
 * The Uptime section's title, and the hidden way to `ResetUptime`. Uptime is
 * self-reported, so the reset is the operator's, and it is for an outage the
 * badge issuer agrees was not the host's fault, such as a Manifold Fedimint
 * Guardian bug.
 */
export const UptimeTitle = () => {
  const [taps, setTaps] = useState(0);
  const reset = useResetUptime();
  const isConfirming = taps >= RESET_UPTIME_TAPS;

  const handleCancel = () => {
    setTaps(0);
    reset.reset();
  };

  const handleConfirm = () => {
    reset.mutate(undefined, { onSuccess: () => setTaps(0) });
  };

  return (
    <>
      <button type="button" className={styles.title} onClick={() => setTaps(taps + 1)}>
        Uptime
      </button>

      {isConfirming && (
        <ResetUptimeConfirm
          onConfirm={handleConfirm}
          onCancel={handleCancel}
          isPending={reset.isPending}
          error={reset.isError ? describeActionError(reset.error) : null}
        />
      )}

      {reset.isSuccess && (
        <span role="status" className={styles.done}>
          Uptime reset. The readiness checks are running again; this page shows their result
          within a minute.
        </span>
      )}
    </>
  );
};
