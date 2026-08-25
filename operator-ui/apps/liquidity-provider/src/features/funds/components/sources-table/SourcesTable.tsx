import { Chip, type ChipTone, type Column, DataTable, SectionCard } from '@operator-ui/common-ui';
import type { InventoryStatus } from '@operator-ui/types';
import type { SourceRow } from '@/features/funds/utils/deriveFunds';
import { formatSats, humanizeToken } from '@/shared/utils/format';

const SOURCE_TONES: Record<InventoryStatus, ChipTone> = {
  available: 'ok',
  unavailable: 'bad',
  disabled: 'neutral',
  unknown: 'warn'
};

const sourceColumns: Column<SourceRow>[] = [
  { key: 'source', header: 'Source', render: (row) => row.source },
  { key: 'available', header: 'Available', render: (row) => formatSats(row.available) },
  {
    key: 'status',
    header: 'Status',
    render: (row) => <Chip tone={SOURCE_TONES[row.status]}>{humanizeToken(row.status)}</Chip>
  }
];

const sourceRowKey = (row: SourceRow) => row.key;

interface SourcesTableProps {
  rows: SourceRow[];
}

export const SourcesTable = ({ rows }: SourcesTableProps) => (
  <SectionCard title="Liquidity sources" frame="table">
    <DataTable columns={sourceColumns} rows={rows} rowKey={sourceRowKey} />
  </SectionCard>
);
