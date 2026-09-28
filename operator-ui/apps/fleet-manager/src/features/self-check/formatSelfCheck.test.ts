import type { SelfCheckResponse } from '@operator-ui/types';
import { describe, expect, it } from 'vitest';
import fixture from '../../../../../packages/types/fixtures/fman_self_check.json';
import {
  formatSelfCheckReport,
  parseSelfCheckResponse,
  summarizeSelfCheck
} from './formatSelfCheck';

describe('self-check shareable output', () => {
  it('accepts the Rust-generated fixture and uses only fixed text', () => {
    const report = parseSelfCheckResponse(fixture);
    expect(report?.state).toBe('completed');
    if (report?.state !== 'completed') throw new Error('fixture is not completed');
    const text = formatSelfCheckReport(report.report);
    const json = JSON.parse(text);
    expect(json.checks[0]).toMatchObject({ id: 'discovery_dns', status: 'pass' });
    expect(json.checks[1].guidance).toBeTruthy();
    expect(json.checks[0].guidance).toBeUndefined();
    expect(text).toContain('Guardian discovery name');
    expect(text).not.toContain('undefined');
  });

  it('only marks all-pass results green, never skipped or unknown results', () => {
    const completed = parseSelfCheckResponse(fixture);
    if (completed?.state !== 'completed') throw new Error('fixture is not completed');
    expect(summarizeSelfCheck(completed.report).tone).toBe('warn');
    const checks = completed.report.checks;
    expect(
      summarizeSelfCheck({
        ...completed.report,
        checks: checks.map((check) => ({ ...check, status: 'pass' })) as typeof checks
      }).tone
    ).toBe('success');
    for (const status of ['not_applicable', 'unknown', 'failure'] as const) {
      expect(
        summarizeSelfCheck({
          ...completed.report,
          checks: checks.map((check, index) => ({
            ...check,
            status: index ? 'pass' : status
          })) as typeof checks
        }).tone
      ).not.toBe('success');
    }
  });

  it('rejects added fields, unknown codes, versions and altered order', () => {
    const completed = fixture as SelfCheckResponse;
    if (completed.state !== 'completed') throw new Error('fixture is not completed');
    expect(parseSelfCheckResponse({ ...completed, secret: 'do-not-copy' })).toBeNull();
    expect(
      parseSelfCheckResponse({ ...completed, report: { ...completed.report, schema_version: 2 } })
    ).toBeNull();
    expect(
      parseSelfCheckResponse({
        ...completed,
        report: {
          ...completed.report,
          checks: [
            { ...completed.report.checks[0], reason_code: 'do-not-copy' },
            ...completed.report.checks.slice(1)
          ]
        }
      })
    ).toBeNull();
    expect(
      parseSelfCheckResponse({
        ...completed,
        report: {
          ...completed.report,
          checks: [
            completed.report.checks[1],
            completed.report.checks[0],
            ...completed.report.checks.slice(2)
          ]
        }
      })
    ).toBeNull();
    expect(parseSelfCheckResponse({ state: 'busy', detail: 'do-not-copy' })).toBeNull();
  });
});
