import { Chip, type ChipTone } from '@operator-ui/common-ui';
import type { ItemAllocationStatus } from '@operator-ui/types';

interface AllocationStatusChipProps {
  status: ItemAllocationStatus;
}

const LABELS: Record<ItemAllocationStatus, string> = {
  pending: 'Pending',
  running: 'Running',
  action_required: 'Action required',
  completed: 'Completed',
  failed: 'Failed',
  cancelled: 'Cancelled'
};

const TONES: Record<ItemAllocationStatus, ChipTone> = {
  pending: 'neutral',
  running: 'info',
  action_required: 'warn',
  completed: 'ok',
  failed: 'bad',
  cancelled: 'neutral'
};

export const AllocationStatusChip = ({ status }: AllocationStatusChipProps) => (
  <Chip tone={TONES[status]}>{LABELS[status]}</Chip>
);
