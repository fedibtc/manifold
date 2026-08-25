import { useEffect, useState } from 'react';
import styles from './CopyStateButton.module.css';

export interface CopyStateButtonProps {
  /** Serialized debug state to place on the clipboard — `store.exportState`. */
  exportState: () => string;
}

type CopyStatus = 'idle' | 'copied' | 'failed';

const LABELS: Record<CopyStatus, string> = {
  idle: 'Copy debug state',
  copied: 'Copied',
  failed: 'Copy failed'
};

export const CopyStateButton = ({ exportState }: CopyStateButtonProps) => {
  const [status, setStatus] = useState<CopyStatus>('idle');

  useEffect(() => {
    if (status === 'idle') return;
    const timer = setTimeout(() => setStatus('idle'), 2000);
    return () => clearTimeout(timer);
  }, [status]);

  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(exportState());
      setStatus('copied');
    } catch {
      // No clipboard permission (or no clipboard API at all, as in jsdom).
      // Saying so beats a silent no-op the developer pastes air from.
      setStatus('failed');
    }
  };

  return (
    <button type="button" className={styles.copy} onClick={handleCopy}>
      {LABELS[status]}
    </button>
  );
};
