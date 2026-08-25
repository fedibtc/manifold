import { SectionCard } from '@operator-ui/common-ui';
import { AttentionRow } from '@/features/overview/components/attention-row/AttentionRow';
import type { AttentionItem } from '@/features/overview/utils/derive';
import styles from './AttentionList.module.css';

interface AttentionListProps {
  items: AttentionItem[];
}

const renderItem = (item: AttentionItem) => <AttentionRow key={item.key} item={item} />;

export const AttentionList = ({ items }: AttentionListProps) => {
  if (items.length === 0) return null;

  return (
    <SectionCard title="Needs attention">
      <ul className={styles.root}>{items.map(renderItem)}</ul>
    </SectionCard>
  );
};
