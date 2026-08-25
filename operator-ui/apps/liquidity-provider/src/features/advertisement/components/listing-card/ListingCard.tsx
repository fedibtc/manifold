import { SectionCard } from '@operator-ui/common-ui';
import styles from './ListingCard.module.css';

interface ListingCardProps {
  provider: string;
  lastPublished: string;
  expires: string;
  sources: string;
  endpoint: string;
}

export const ListingCard = ({
  provider,
  lastPublished,
  expires,
  sources,
  endpoint
}: ListingCardProps) => (
  <SectionCard title="Listing">
    <dl className={styles.kv}>
      <dt className={styles.key}>Provider</dt>

      <dd className={styles.value}>
        <span className={styles.mono}>{provider}</span>
      </dd>

      <dt className={styles.key}>Last published</dt>

      <dd className={styles.value}>{lastPublished}</dd>

      <dt className={styles.key}>Expires</dt>

      <dd className={styles.value}>{expires}</dd>

      <dt className={styles.key}>Sources offered</dt>

      <dd className={styles.value}>{sources}</dd>

      <dt className={styles.key}>Endpoint</dt>

      <dd className={styles.value}>
        <span className={styles.mono}>{endpoint}</span>
      </dd>
    </dl>

    <p className={styles.note}>
      The public listing never includes your balances or federation details — only what's above.
    </p>
  </SectionCard>
);
