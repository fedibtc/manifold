import styles from './CopyButton.module.css';
import { useCopyToClipboard } from './useCopyToClipboard';

interface CopyButtonProps {
  value: string;
  label: string;
  /** Show `label` on screen too. For a control that does not sit beside the
   *  value it copies, where the icon alone cannot say which value that is. */
  showLabel?: boolean;
}

export const CopyButton = ({ value, label, showLabel = false }: CopyButtonProps) => {
  const { copied, failed, copy } = useCopyToClipboard();
  const handleClick = () => copy(value);

  return (
    <button
      type="button"
      className={styles.root}
      data-copied={copied}
      data-failed={failed}
      data-labelled={showLabel}
      onClick={handleClick}
      aria-label={failed ? `${label} — copying failed, select the text instead` : label}
      title={failed ? 'Copying failed. Select the text and copy it yourself.' : undefined}
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
      {showLabel && <span className={styles.label}>{label}</span>}
    </button>
  );
};
