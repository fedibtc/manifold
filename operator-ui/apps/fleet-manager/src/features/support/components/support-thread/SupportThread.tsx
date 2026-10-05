import type { SupportMessage } from '@operator-ui/types';
import { useEffect, useRef } from 'react';
import { formatSupportTimestamp } from '@/features/support/utils/supportMessage';
import styles from './SupportThread.module.css';

// The Fedi app's chat starts a new group, under its own time, when a message
// comes more than a minute after the one before it.
const GROUP_GAP_SECONDS = 60;

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
          const previous = messages[index - 1];
          const newGroup =
            !previous || message.created_at - previous.created_at > GROUP_GAP_SECONDS;
          const spacing = newGroup
            ? undefined
            : previous.author === message.author
              ? 'run'
              : 'turn';
          return (
            <li
              key={message.id}
              className={styles.item}
              data-author={message.author}
              data-spacing={spacing}
            >
              {newGroup && (
                <time
                  className={styles.time}
                  dateTime={new Date(message.created_at * 1000).toISOString()}
                >
                  {formatSupportTimestamp(message.created_at)}
                </time>
              )}

              <p className={styles.bubble}>
                <span className={styles.srOnly}>
                  {message.author === 'fedi' ? 'Fedi support: ' : 'You: '}
                </span>
                {message.body}
              </p>
            </li>
          );
        })}
      </ol>

      <div ref={end} />
    </div>
  );
};
