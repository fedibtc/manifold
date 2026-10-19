import authorizeApplicationRow from '@/shared/assets/peerbadge/authorize-application-row.webp';
import { PeerBadgeCallout } from '@/shared/components/peerbadge-callout/PeerBadgeCallout';
import styles from './AuthorizeApplicationCallout.module.css';

export const AuthorizeApplicationCallout = () => (
  <div className={styles.root}>
    <PeerBadgeCallout
      src={authorizeApplicationRow}
      alt="Authorize application: let an app use only this badge"
    />
  </div>
);
