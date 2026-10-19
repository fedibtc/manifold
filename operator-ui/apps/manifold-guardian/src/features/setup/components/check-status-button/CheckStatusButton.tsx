import { Button } from '@operator-ui/common-ui';
import styles from './CheckStatusButton.module.css';

interface CheckStatusButtonProps {
  loading: boolean;
  onClick: () => void;
}

export const CheckStatusButton = ({ loading, onClick }: CheckStatusButtonProps) => (
  <Button variant="secondary" size="small" loading={loading} onClick={onClick}>
    <svg className={styles.icon} viewBox="0 0 24 24" aria-hidden="true">
      <path d="M20 12a8 8 0 0 1-14.3 4.9" />

      <path d="M4 12a8 8 0 0 1 14.3-4.9" />

      <path d="M18.5 3.5v3.8h-3.8" />

      <path d="M5.5 20.5v-3.8h3.8" />
    </svg>
    Check status
  </Button>
);
