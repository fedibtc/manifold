import { Chip, type ChipTone } from '@operator-ui/common-ui';
import { Link } from 'react-router-dom';
import type { AttentionItem, Severity } from '@/features/overview/utils/derive';
import styles from './AttentionRow.module.css';

interface AttentionRowProps {
  item: AttentionItem;
}

const severityBadge: Record<Severity, { tone: ChipTone; label: string }> = {
  critical: { tone: 'bad', label: 'Unhealthy' },
  warning: { tone: 'warn', label: 'Warning' }
};

export const AttentionRow = ({ item }: AttentionRowProps) => {
  const badge = severityBadge[item.severity];

  return (
    <li className={styles.root}>
      <Chip tone={badge.tone}>{badge.label}</Chip>

      <div className={styles.body}>
        <span className={styles.title}>{item.title}</span>

        <span className={styles.detail}>{item.detail}</span>
      </div>
      {item.action ? (
        <Link to={item.action.path} className={styles.action}>
          {item.action.label}
        </Link>
      ) : null}
    </li>
  );
};
