import type {
  AdminAllocationDetail,
  AdminAllocationSummary,
  ItemAllocationStatus
} from '@operator-ui/types';

// The federation-centric admin API reports allocation progress per source
// (gateway / stability pool) and per item, with no single overall field. The UI
// still shows one headline status, so collapse the parts, surfacing the most
// attention-needing state first.
const PRIORITY: ItemAllocationStatus[] = [
  'failed',
  'action_required',
  'pending',
  'running',
  'cancelled',
  'completed'
];

export const collapseAllocationStatus = (
  statuses: (ItemAllocationStatus | null | undefined)[]
): ItemAllocationStatus => {
  const present = new Set(
    statuses.filter((status): status is ItemAllocationStatus => Boolean(status))
  );
  return PRIORITY.find((status) => present.has(status)) ?? 'pending';
};

export const summaryStatus = (summary: AdminAllocationSummary): ItemAllocationStatus =>
  collapseAllocationStatus([summary.gateway_status, summary.stability_pool_status]);

export const detailStatus = (detail: AdminAllocationDetail): ItemAllocationStatus =>
  collapseAllocationStatus(detail.status.item_statuses.map((item) => item.status));
