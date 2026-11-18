import { Banner } from '@operator-ui/common-ui';
import { Link } from 'react-router-dom';
import { markWhatsNewSeen, type WhatsNewItem } from '@/shared/whats-new/whatsNew';
import styles from './WhatsNew.module.css';

interface WhatsNewProps {
  /** What applies now, newest first; each item drops out once seen. */
  items: WhatsNewItem[];
}

/** Announcements, stacked so none hides behind another. */
export const WhatsNew = ({ items }: WhatsNewProps) => {
  if (items.length === 0) return null;
  return (
    <section aria-label="What's new" className={styles.root}>
      {items.map((item) => (
        <Banner
          key={item.id}
          variant="info"
          title={item.title}
          action={
            <div className={styles.actions}>
              <button
                type="button"
                className={styles.notNow}
                onClick={() => markWhatsNewSeen(item.id)}
              >
                Not now
              </button>

              <Link to={item.action.path} className={styles.primary}>
                {item.action.label}
              </Link>
            </div>
          }
        >
          {item.body}
        </Banner>
      ))}
    </section>
  );
};
