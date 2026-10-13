import { Link } from 'react-router-dom';
import type { AttentionItem } from '@/features/overview/utils/deriveOverview';
import styles from './AttentionRow.module.css';

interface AttentionRowProps {
  item: AttentionItem;
}

export const AttentionRow = ({ item }: AttentionRowProps) => (
  <li className={styles.root}>
    <div className={styles.body}>
      <span className={styles.title}>{item.title}</span>

      <span className={styles.detail}>{item.detail}</span>
    </div>

    <Link to={item.path} className={styles.action}>
      Review
    </Link>
  </li>
);
