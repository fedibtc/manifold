import myBadgesRow from '@/shared/assets/peerbadge/my-badges-row.webp';
import { PeerBadgeCallout } from '@/shared/components/peerbadge-callout/PeerBadgeCallout';
import styles from './MyBadgesCallout.module.css';

export const MyBadgesCallout = () => (
  <div className={styles.root}>
    <PeerBadgeCallout
      src={myBadgesRow}
      alt="My badges: manage private badges, publishing, and app authorizations"
    />
  </div>
);
