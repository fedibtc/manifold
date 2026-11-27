import { Button, Chip, type ChipTone, type Column, DataTable } from '@operator-ui/common-ui';
import type { SeatGrantEntry, SeatGrantStatus } from '@operator-ui/types';
import { Link } from 'react-router-dom';
import {
  isShareable,
  type ShareableGrant
} from '@/features/guardian-codes/components/guardian-code-share/GuardianCodeShare';
import { formatDate } from '@/shared/utils/format';
import styles from './GuardianCodeList.module.css';

// Quiet by default: nothing here is a problem, so only the label tells the
// three states apart.
const STATUS: Record<SeatGrantStatus['state'], { label: string; tone: ChipTone }> = {
  unused: { label: 'Unused', tone: 'info' },
  used: { label: 'Used', tone: 'neutral' },
  revoked: { label: 'Revoked', tone: 'neutral' }
};

interface GuardianCodeListProps {
  grants: SeatGrantEntry[];
  onShow: (grant: ShareableGrant) => void;
  onRevoke: (grant: SeatGrantEntry) => void;
}

export const GuardianCodeList = ({ grants, onShow, onRevoke }: GuardianCodeListProps) => {
  const columns: Column<SeatGrantEntry>[] = [
    {
      key: 'label',
      header: 'Label',
      render: (grant) =>
        grant.label ? (
          <span className={styles.label}>{grant.label}</span>
        ) : (
          <span className={styles.noLabel}>No label</span>
        )
    },
    { key: 'created', header: 'Created', render: (grant) => formatDate(grant.issued_at * 1000) },
    {
      key: 'status',
      header: 'Status',
      render: (grant) => (
        <Chip tone={STATUS[grant.status.state].tone}>{STATUS[grant.status.state].label}</Chip>
      )
    },
    {
      key: 'actions',
      header: '',
      render: (grant) => {
        if (isShareable(grant)) {
          return (
            <span className={styles.actions}>
              <Button variant="secondary" size="small" onClick={() => onShow(grant)}>
                Show code
              </Button>

              <Button variant="secondary" size="small" onClick={() => onRevoke(grant)}>
                Revoke
              </Button>
            </span>
          );
        }
        if (grant.status.state === 'used') {
          return (
            <Link to={`/seats/${grant.status.seat_id}`} className={styles.seatLink}>
              View seat
            </Link>
          );
        }
        return null;
      }
    }
  ];

  return (
    <div className={styles.tableWrap}>
      <DataTable columns={columns} rows={grants} rowKey={(grant) => grant.grant_nonce} />
    </div>
  );
};
