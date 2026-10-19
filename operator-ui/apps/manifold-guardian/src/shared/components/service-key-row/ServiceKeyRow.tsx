import { CopyButton } from '@operator-ui/common-ui';
import styles from './ServiceKeyRow.module.css';

interface ServiceKeyRowProps {
  pubkey: string;
}

// The key is shown whole, unlike BackupPage, which truncates the same class
// of value. That page lists keys for reference; this one shows the single
// value a holder may compare against their own application, and a truncated
// value does not permit that comparison.
export const ServiceKeyRow = ({ pubkey }: ServiceKeyRowProps) => (
  <div className={styles.keyRow}>
    <code className={styles.key}>{pubkey}</code>

    <CopyButton value={pubkey} label="Copy the service Nostr public key" />
  </div>
);
