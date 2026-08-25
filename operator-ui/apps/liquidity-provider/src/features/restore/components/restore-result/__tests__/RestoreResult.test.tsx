import type { RestoreBackupResponse } from '@operator-ui/types';
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { RestoreResult } from '../RestoreResult';

const result: RestoreBackupResponse = {
  status: 'ready',
  validation: {
    status: 'failed',
    checks: [{ name: 'wallet_check', status: 'failed', detail: 'balance mismatch' }]
  },
  restored_state_groups: ['database', 'operator_config']
} as unknown as RestoreBackupResponse;

describe('RestoreResult', () => {
  it('should render the status, failed checks and restored state groups', () => {
    render(<RestoreResult result={result} />);

    expect(screen.getByText('Status: ready')).toBeTruthy();
    expect(screen.getByText(/wallet_check/)).toBeTruthy();
    expect(screen.getByText('database')).toBeTruthy();
    expect(screen.getByText('operator_config')).toBeTruthy();
  });

  it('should render the restart instruction', () => {
    render(<RestoreResult result={result} />);

    expect(screen.getByText(/Restart the daemon/)).toBeTruthy();
  });

  it('should show an all-passed banner when no checks failed', () => {
    render(
      <RestoreResult
        result={
          {
            status: 'ready',
            validation: { status: 'passed', checks: [] },
            restored_state_groups: ['database']
          } as unknown as RestoreBackupResponse
        }
      />
    );

    expect(screen.getByText('All validation checks passed.')).toBeTruthy();
  });
});
