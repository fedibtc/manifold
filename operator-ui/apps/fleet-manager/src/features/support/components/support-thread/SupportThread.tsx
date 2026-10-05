import type { SupportMessage } from '@operator-ui/types';
import { useEffect, useRef } from 'react';
import { formatSupportDay, formatSupportTime } from '@/features/support/utils/supportMessage';
import styles from './SupportThread.module.css';

export const SupportThread = ({ messages }: { messages: SupportMessage[] }) => {
  const end = useRef<HTMLDivElement>(null);
  const latest = messages.at(-1)?.id;

  // Open at, and follow, the newest message.
  useEffect(() => {
    if (latest) end.current?.scrollIntoView?.({ block: 'end' });
  }, [latest]);

  if (messages.length === 0) {
    return <p className={styles.empty}>No messages yet. Write to Fedi below.</p>;
  }

  return (
    <div className={styles.scroller}>
      <ol className={styles.thread}>
        {messages.map((message, index) => {
          const day = formatSupportDay(message.created_at);
          const previous = messages[index - 1];
          const firstOfDay = !previous || formatSupportDay(previous.created_at) !== day;
          return (
            <li key={message.id} className={styles.item} data-author={message.author}>
              {firstOfDay && <span className={styles.day}>{day}</span>}

              <div className={styles.bubble}>
                <p className={styles.body}>{message.body}</p>

                <span className={styles.meta}>
                  {formatSupportTime(message.created_at)}
                  {message.author === 'fedi' && ' · Fedi support'}
                </span>
              </div>
            </li>
          );
        })}
      </ol>

      <div ref={end} />
    </div>
  );
};
