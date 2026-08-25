import type { ItemAllocationStatus } from '@operator-ui/types';
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { AllocationStatusChip } from '@/features/allocations/components/allocation-status-chip/AllocationStatusChip';

const CASES: [ItemAllocationStatus, string][] = [
  ['pending', 'Pending'],
  ['running', 'Running'],
  ['action_required', 'Action required'],
  ['completed', 'Completed'],
  ['failed', 'Failed'],
  ['cancelled', 'Cancelled']
];

describe('AllocationStatusChip', () => {
  it.each(CASES)('should render the human label for %s', (status, label) => {
    render(<AllocationStatusChip status={status} />);
    expect(screen.getByText(label)).toBeTruthy();
  });

  it('should expose the Fedi chip tone as a data attribute', () => {
    render(<AllocationStatusChip status="running" />);
    expect(screen.getByText('Running').getAttribute('data-tone')).toBe('info');
  });
});
