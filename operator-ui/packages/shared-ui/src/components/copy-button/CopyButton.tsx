import styles from './CopyButton.module.css';
import { useCopyToClipboard } from './useCopyToClipboard';

interface CopyButtonProps {
  value: string;
  label: string;
}

export const CopyButton = ({ value, label }: CopyButtonProps) => {
  const { copied, copy } = useCopyToClipboard();
  const handleClick = () => copy(value);

  return (
    <button
      type="button"
      className={styles.root}
      data-copied={copied}
      onClick={handleClick}
      aria-label={label}
    >
      {copied ? (
        <svg viewBox="0 0 24 24" className={styles.icon} aria-hidden="true">
          <path
            d="M20 6 9 17l-5-5"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        </svg>
      ) : (
        <svg viewBox="0 0 24 24" className={styles.icon} aria-hidden="true">
          <rect
            x="9"
            y="9"
            width="13"
            height="13"
            rx="2"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
          />

          <path
            d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        </svg>
      )}
    </button>
  );
};
