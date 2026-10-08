import { useEffect, useId } from 'react';
import { useMarkSupportRead } from '@/features/support/api/hooks/use-mark-support-read/useMarkSupportRead';
import { useSupportChat } from '@/features/support/api/hooks/use-support-chat/useSupportChat';
import { FediAvatar } from '@/features/support/components/fedi-avatar/FediAvatar';
import { SupportComposer } from '@/features/support/components/support-composer/SupportComposer';
import { SupportThread } from '@/features/support/components/support-thread/SupportThread';
import { SUPPORT_CHAT_OPEN_POLL_MS } from '@/shared/api/pollingIntervals';
import { QuerySurface } from '@/shared/components/query-surface/QuerySurface';
import { useQueryDisposition } from '@/shared/query/use-query-disposition/useQueryDisposition';
import styles from './SupportPage.module.css';

export const SupportPage = () => {
  const chat = useSupportChat(SUPPORT_CHAT_OPEN_POLL_MS);
  const markRead = useMarkSupportRead();
  const { disposition, retry } = useQueryDisposition([chat]);
  const titleId = useId();

  // Reading the page reads the unread Fedi messages it shows, and only those.
  // Each poll tries again, so one failed mark does not leave the page unread.
  const unreadIds = chat.data?.available
    ? chat.data.messages.filter((message) => message.unread).map((message) => message.id)
    : [];
  const unreadKey = unreadIds.join(' ');
  const { mutate } = markRead;
  const polledAt = chat.dataUpdatedAt;
  useEffect(() => {
    if (polledAt > 0 && unreadKey !== '') mutate(unreadKey.split(' '));
  }, [polledAt, unreadKey, mutate]);

  return (
    <div className={styles.root}>
      <div className={styles.pageHead}>
        <h1 className={styles.heading}>Support</h1>
      </div>

      <QuerySurface disposition={disposition} onRetry={retry}>
        {chat.data && (
          <section className={styles.card} aria-labelledby={titleId}>
            <header className={styles.cardHead}>
              <FediAvatar />

              <h2 id={titleId} className={styles.cardTitle}>
                Fedi guardian support
              </h2>
            </header>

            <p className={styles.safety}>
              <svg aria-hidden="true" viewBox="0 0 24 24" className={styles.safetyIcon}>
                <path d="M12 3 5 6v5c0 4.5 3 8.3 7 10 4-1.7 7-5.5 7-10V6l-7-3z" />

                <path d="m9 12 2 2 4-4" />
              </svg>
              Fedi support will never ask for your recovery phrase, your password or remote access
              to your machine.
            </p>

            {chat.data.available ? (
              <>
                <SupportThread messages={chat.data.messages} />

                <SupportComposer />
              </>
            ) : (
              <p className={styles.unavailable}>
                Chat with Fedi support isn't available on this host yet.
              </p>
            )}
          </section>
        )}
      </QuerySurface>
    </div>
  );
};
