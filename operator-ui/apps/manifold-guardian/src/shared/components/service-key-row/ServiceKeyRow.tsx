import { CopyButton } from '@operator-ui/common-ui';
import styles from './ServiceKeyRow.module.css';

interface ServiceKeyRowProps {
  pubkey: string;
}

export const ServiceKeyRow = ({ pubkey }: ServiceKeyRowProps) => (
  <div className={styles.keyRow}>
    <code className={styles.key}>{pubkey}</code>

    <CopyButton value={pubkey} label="Copy the service Nostr public key" />
  </div>
);
