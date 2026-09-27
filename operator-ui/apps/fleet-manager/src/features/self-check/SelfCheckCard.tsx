import type { SelfCheckResponse } from '@operator-ui/types';
import { useMutation } from '@tanstack/react-query';
import { useEffect, useRef, useState } from 'react';
import { adminCall } from '@/shared/api/adminCall';
import { formatSelfCheckReport, parseSelfCheckResponse } from './formatSelfCheck';
import styles from './SelfCheckCard.module.css';

export const SelfCheckCard = () => {
  const [report, setReport] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [manualCopy, setManualCopy] = useState(false);
  const text = useRef<HTMLTextAreaElement>(null);
  const run = useMutation({
    mutationFn: () => adminCall<unknown>('RunSelfCheck'),
    retry: false,
    gcTime: 0,
    onSuccess: (payload) => {
      const response = parseSelfCheckResponse(payload);
      if (!response) {
        setNotice(
          'This daemon returned an unsupported self-check report. Check for a compatible update.'
        );
      } else if (response.state === 'completed') {
        setReport(formatSelfCheckReport(response.report));
      } else {
        const notices: Record<Exclude<SelfCheckResponse['state'], 'completed'>, string> = {
          busy: 'A self-check is already running. Try again after it finishes.',
          cooldown: 'A self-check ran recently. Wait a little before trying again.',
          unavailable: 'Self-check is unavailable while this daemon is stopping.'
        };
        setNotice(notices[response.state]);
      }
    },
    onError: () =>
      setNotice(
        'Self-check is unavailable on this daemon or the request failed. Check the connection and try again.'
      )
  });
  useEffect(() => () => run.reset(), [run.reset]);

  const start = () => {
    setReport(null);
    setNotice(null);
    setManualCopy(false);
    run.mutate();
  };
  const copy = async () => {
    if (report === null) return;
    try {
      await navigator.clipboard.writeText(report);
      setManualCopy(false);
      setNotice('Report copied. Review it before sharing.');
    } catch {
      setManualCopy(true);
      setNotice('Clipboard unavailable. Select and copy the report manually.');
      text.current?.focus();
      text.current?.select();
    }
  };
  return (
    <section className={styles.card} aria-label="Self-check">
      <h2>Self-check</h2>

      <p>
        Checks contact configured services. They do not change settings or upload a report. Results
        contain generic outcomes and guidance; review the preview before sharing.
      </p>

      <button type="button" disabled={run.isPending} onClick={start}>
        {run.isPending ? 'Checking…' : 'Run self-check'}
      </button>
      {notice && <p role="status">{notice}</p>}
      {report !== null && (
        <div>
          <label htmlFor="self-check-report">Report preview</label>

          <textarea
            id="self-check-report"
            ref={text}
            className={styles.preview}
            readOnly
            value={report}
          />

          <button type="button" onClick={copy}>
            Copy report
          </button>
          {manualCopy && <p>Select and copy the report manually.</p>}
        </div>
      )}
    </section>
  );
};
