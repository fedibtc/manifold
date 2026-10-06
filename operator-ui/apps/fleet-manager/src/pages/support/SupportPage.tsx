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

  // Reading the page reads every Fedi message it shows, and only those: one
  // that arrives after this render stays unread until the page shows it.
  const lastFromFedi = chat.data?.messages.filter((message) => message.author === 'fedi').at(-1);
  const unread = chat.data?.unread ?? 0;
  const { mutate } = markRead;
  // Each poll tries again, so one failed mark does not leave the page unread.
  const polledAt = chat.dataUpdatedAt;
  useEffect(() => {
    if (polledAt > 0 && unread > 0 && lastFromFedi) mutate(lastFromFedi.id);
  }, [polledAt, unread, lastFromFedi, mutate]);

  return (
    <div className={styles.root}>
      <div className={styles.pageHead}>
        <h1 className={styles.heading}>Support</h1>

        <p className={styles.intro}>
          Talk to the Fedi guardian team. Messages are end-to-end encrypted over Nostr. Fedi sees
          this host's public key and what you write.
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
                  Fedi support chat is not available for this deployment yet.
                </p>
              )}
            </div>
          </SectionCard>
        )}
      </QuerySurface>
    </div>
  );
};
