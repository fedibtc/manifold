import { StaleDataBanner } from '@operator-ui/common-ui';
import { useState } from 'react';
import { useAllocations } from '@/features/allocations/api/hooks/use-allocations/useAllocations';
import { AllocationsTable } from '@/features/allocations/components/allocations-table/AllocationsTable';
import { TimelinePanel } from '@/features/allocations/components/timeline-panel/TimelinePanel';
import styles from './AllocationsPage.module.css';

const PageHeader = () => (
  <>
    <h1 className={styles.heading}>Allocations</h1>

    <p className={styles.subtitle}>
      Funding jobs created from accepted requests. On-chain steps can take hours — that&apos;s
      normal.
    </p>
  </>
);

export const AllocationsPage = () => {
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const allocationsQuery = useAllocations();

  const handleSelect = (federationId: string) => setSelectedId(federationId);

  // The list polls every 5s while anything is in flight. A failed poll must not
  // empty a table the operator is reading, so the error state is reserved for
  // having no list at all; a failure over cached rows keeps them under a stale
  // banner.
  if (!allocationsQuery.data) {
    return (
      <>
        <PageHeader />

        {allocationsQuery.isError ? (
          <p className={styles.state} data-tone="error">
            Could not load allocations.
          </p>
        ) : (
          <p className={styles.state}>Loading allocations…</p>
        )}
      </>
    );
  }

  return (
    <>
      <PageHeader />

      {allocationsQuery.isError && <StaleDataBanner updatedAtMs={allocationsQuery.dataUpdatedAt} />}

      <AllocationsTable
        rows={allocationsQuery.data.allocations.items}
        selectedId={selectedId}
        onSelect={handleSelect}
      />

      {selectedId ? <TimelinePanel allocationId={selectedId} /> : null}
    </>
  );
};
