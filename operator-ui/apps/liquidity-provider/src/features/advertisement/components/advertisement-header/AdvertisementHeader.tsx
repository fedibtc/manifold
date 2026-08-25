import type { ReactNode } from 'react';
import styles from './AdvertisementHeader.module.css';

interface AdvertisementHeaderProps {
  status?: ReactNode;
}

export const AdvertisementHeader = ({ status }: AdvertisementHeaderProps) => (
  <header className={styles.head}>
    <div>
      <h1 className={styles.heading}>Advertisement</h1>

      <p className={styles.sub}>What federations see when they look for liquidity on Nostr.</p>
    </div>
    {status}
  </header>
);
