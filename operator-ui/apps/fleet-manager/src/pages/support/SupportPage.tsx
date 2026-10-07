import { Banner, SectionCard } from '@operator-ui/common-ui';
import { useEffect } from 'react';
import { useMarkSupportRead } from '@/features/support/api/hooks/use-mark-support-read/useMarkSupportRead';
import { useSupportChat } from '@/features/support/api/hooks/use-support-chat/useSupportChat';
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

        <p className={styles.intro}>
          Ask the Fedi team for help with this host. Only you and Fedi can read these messages. Fedi
          sees which host wrote and what you wrote.
        </p>
      </div>

      <QuerySurface disposition={disposition} onRetry={retry}>
        {chat.data && (
          <SectionCard title="Fedi guardian support">
            <div className={styles.chat}>
              <Banner
                variant="info"
                title="Fedi support will never ask for your recovery phrase, your password or remote access to your machine."
              >
                If someone does, it is not Fedi. Stop the chat.
              </Banner>

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
            </div>
          </SectionCard>
        )}
      </QuerySurface>
    </div>
  );
};
