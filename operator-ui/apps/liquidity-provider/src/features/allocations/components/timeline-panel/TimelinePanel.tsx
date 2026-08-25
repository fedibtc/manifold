import { SectionCard, StaleDataBanner } from '@operator-ui/common-ui';
import type { ReactNode } from 'react';
import { useAllocation } from '@/features/allocations/api/hooks/use-allocation/useAllocation';
import { AllocationTimeline } from '@/features/allocations/components/allocation-timeline/AllocationTimeline';
import styles from './TimelinePanel.module.css';

interface TimelinePanelProps {
  allocationId: string;
}

export const TimelinePanel = ({ allocationId }: TimelinePanelProps) => {
  const detailQuery = useAllocation(allocationId);

  // Detail polls at the same 5s cadence as the list. A failed poll keeps the
  // timeline on screen under a stale banner rather than collapsing the panel
  // an operator is mid-way through reading.
  let content: ReactNode;
  if (detailQuery.data) {
    content = (
      <>
        {detailQuery.isError && <StaleDataBanner updatedAtMs={detailQuery.dataUpdatedAt} />}

        <AllocationTimeline detail={detailQuery.data.allocation} />
      </>
    );
  } else if (detailQuery.isError) {
    content = (
      <p className={styles.state} data-tone="error">
        Could not load timeline.
      </p>
    );
  } else {
    content = <p className={styles.state}>Loading timeline…</p>;
  }

  return (
    <div className={styles.detail}>
      <SectionCard title={`In flight — ${allocationId}`}>{content}</SectionCard>
    </div>
  );
};
