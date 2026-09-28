import { Banner } from '@operator-ui/common-ui';
import type { SelfCheckResponse } from '@operator-ui/types';
import { useMutation } from '@tanstack/react-query';
import { useEffect, useRef, useState } from 'react';
import { adminCall } from '@/shared/api/adminCall';
import {
  formatSelfCheckReport,
  guidance,
  labels,
  parseSelfCheckResponse,
  reasons,
  statuses,
  summarizeSelfCheck
} from './formatSelfCheck';
import styles from './SelfCheckCard.module.css';

export const SelfCheckCard = () => {
  const [result, setResult] = useState<
    Extract<SelfCheckResponse, { state: 'completed' }>['report'] | null
  >(null);
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
        setResult(response.report);
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
    setResult(null);
    setNotice(null);
    setManualCopy(false);
    run.mutate();
  };
  const copy = async () => {
    if (result === null) return;
    const report = formatSelfCheckReport(result);
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
  const summary = result && summarizeSelfCheck(result);
  const report = result && formatSelfCheckReport(result);
  return (
    <section className={styles.card} aria-label="Local health check">
      <h2>Local health check</h2>

      <p>
        Check this host’s configured services and cached observations. Nothing is changed or
        uploaded.
      </p>

      <button type="button" disabled={run.isPending} onClick={start}>
        {run.isPending ? 'Checking…' : 'Run self-check'}
      </button>
      {run.isPending && <p role="status">Checking configured services…</p>}
      {notice && <p role="status">{notice}</p>}
      {result && summary && (
        <div className={styles.results} aria-live="polite">
          <Banner
            variant={summary.tone}
            title={
              summary.tone === 'success'
                ? 'All checks passed'
                : summary.tone === 'error'
                  ? 'Problem detected'
                  : summary.tone === 'warn'
                    ? 'Needs attention'
                    : 'Some checks were not confirmed'
            }
          >
            {summary.tone === 'success'
              ? `${summary.counts.pass} checks passed.`
              : [
                  `${summary.counts.pass} passed`,
                  summary.counts.warning && `${summary.counts.warning} warnings`,
                  summary.counts.failure && `${summary.counts.failure} failures`,
                  summary.counts.unknown && `${summary.counts.unknown} unknown`,
                  summary.counts.not_applicable && `${summary.counts.not_applicable} not applicable`
                ]
                  .filter(Boolean)
                  .join(' · ')}
          </Banner>
          {summary.tone !== 'success' && (
            <ul className={styles.findings}>
              {result.checks
                .filter((check) => check.status !== 'pass')
                .map((check) => (
                  <li key={check.check_id} className={styles.finding}>
                    <strong>
                      {labels[check.check_id]} — {statuses[check.status]}
                    </strong>

                    <span>{reasons[check.reason_code]}</span>
                    {(check.status === 'warning' ||
                      check.status === 'failure' ||
                      check.status === 'unknown') && <span>{guidance[check.check_id]}</span>}
                  </li>
                ))}
            </ul>
          )}
          <details>
            <summary>Shareable report</summary>

            <p>Fixed-vocabulary results only. Review before sharing.</p>

            <label htmlFor="self-check-report">Report preview</label>

            <textarea
              id="self-check-report"
              ref={text}
              className={styles.preview}
              readOnly
              value={report ?? ''}
            />

            <button type="button" onClick={copy}>
              Copy report
            </button>
            {manualCopy && <p>Select and copy the report manually.</p>}
          </details>
        </div>
      )}
    </section>
  );
};
