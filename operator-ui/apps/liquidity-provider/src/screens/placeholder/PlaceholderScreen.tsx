import { SectionCard } from '@operator-ui/common-ui';
import styles from './PlaceholderScreen.module.css';

interface PlaceholderScreenProps {
  title: string;
}

export const PlaceholderScreen = ({ title }: PlaceholderScreenProps) => (
  <>
    <h1 className={styles.heading}>{title}</h1>

    <SectionCard title={title}>Coming soon.</SectionCard>
  </>
);
