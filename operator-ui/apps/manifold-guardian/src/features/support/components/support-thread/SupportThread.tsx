import type { SupportMessage } from '@operator-ui/types';
import { useEffect, useRef } from 'react';
import {
  formatSupportDay,
  formatSupportTime,
  isSameSupportDay
} from '@/features/support/utils/supportMessage';
import styles from './SupportThread.module.css';

interface SupportThreadProps {
  messages: SupportMessage[];
}

export const SupportThread = ({ messages }: SupportThreadProps) => {
  const end = useRef<HTMLDivElement>(null);
  const firstUnreadItem = useRef<HTMLLIElement>(null);
  const latest = messages.at(-1)?.id;
  const firstUnread = messages.find((message) => message.unread)?.id;

  // Open at, and follow, the newest message, but show an unread message
  // first: one that arrives late can sort above the bottom.
  useEffect(() => {
    if (latest) end.current?.scrollIntoView?.({ block: 'end' });
  }, [latest]);

  useEffect(() => {
    if (firstUnread) firstUnreadItem.current?.scrollIntoView?.({ block: 'nearest' });
  }, [firstUnread]);

  if (messages.length === 0) {
    return <p className={styles.empty}>No messages yet. Write to guardian support below.</p>;
  }

  return (
    <div className={styles.scroller}>
      <ol className={styles.thread}>
        {messages.map((message, index) => {
          const previous = messages[index - 1];
          const newDay = !previous || !isSameSupportDay(previous.created_at, message.created_at);
          const fromFedi = message.author === 'fedi';
          return (
            <li
              key={message.id}
              ref={message.id === firstUnread ? firstUnreadItem : undefined}
              className={styles.item}
              data-author={message.author}
            >
              {newDay && <p className={styles.day}>{formatSupportDay(message.created_at)}</p>}

              <div className={styles.row}>
                <div className={styles.bubble}>
                  <p className={styles.body}>
                    <span className={styles.srOnly}>
                      {fromFedi ? 'Guardian support: ' : 'You: '}
                    </span>
                    {message.body}
                  </p>

                  <time
                    className={styles.time}
                    dateTime={new Date(message.created_at * 1000).toISOString()}
                  >
                    {formatSupportTime(message.created_at)}
                  </time>
                </div>
              </div>
            </li>
          );
        })}
      </ol>

      <div ref={end} />
    </div>
  );
};
