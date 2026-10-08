import fediLogo from '@/features/support/components/fedi-avatar/fediLogo.svg';
import styles from './FediAvatar.module.css';

// Fedi support's avatar, as the operator design system's `sup-av`.
export const FediAvatar = () => (
  <span className={styles.root}>
    <img src={fediLogo} alt="" />
  </span>
);
