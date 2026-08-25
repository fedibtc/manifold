import { truncateMiddle } from '@operator-ui/common-ui';
import styles from './AllocationIdButton.module.css';

interface AllocationIdButtonProps {
  id: string;
  selected: boolean;
  onSelect: (id: string) => void;
}

export const AllocationIdButton = ({ id, selected, onSelect }: AllocationIdButtonProps) => {
  const handleClick = () => onSelect(id);
  return (
    <button
      type="button"
      className={styles.root}
      data-selected={selected}
      aria-pressed={selected}
      onClick={handleClick}
    >
      {truncateMiddle(id, 8, 8)}
    </button>
  );
};
