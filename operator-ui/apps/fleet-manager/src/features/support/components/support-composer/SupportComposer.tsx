import { Button } from '@operator-ui/common-ui';
import { type FormEvent, type KeyboardEvent, useId, useState } from 'react';
import { useSendSupportMessage } from '@/features/support/api/hooks/use-send-support-message/useSendSupportMessage';
import {
  MAX_SUPPORT_MESSAGE_CHARS,
  supportMessageLength
} from '@/features/support/utils/supportMessage';
import { describeActionError } from '@/shared/utils/describeActionError';
import styles from './SupportComposer.module.css';

// Enter sends and Shift+Enter starts a new line, as in other chats. A failed
// send keeps the text, so sending again is the retry.
export const SupportComposer = () => {
  const id = useId();
  const send = useSendSupportMessage();
  const [body, setBody] = useState('');
  const [validationError, setValidationError] = useState<string | null>(null);

  const submit = () => {
    const length = supportMessageLength(body);
    if (length === 0) return;
    if (length > MAX_SUPPORT_MESSAGE_CHARS) {
      setValidationError(`A message can have at most ${MAX_SUPPORT_MESSAGE_CHARS} characters.`);
      return;
    }
    send.mutate(body.trim(), { onSuccess: () => setBody('') });
  };

  const handleSubmit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    submit();
  };

  const handleKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) {
      event.preventDefault();
      submit();
    }
  };

  const error = validationError ?? (send.isError ? describeActionError(send.error) : null);

  return (
    <form className={styles.form} onSubmit={handleSubmit}>
      <div className={styles.row}>
        <label className={styles.srOnly} htmlFor={id}>
          Message to Fedi support
        </label>

        <textarea
          id={id}
          className={styles.textarea}
          rows={2}
          placeholder="Write a message…"
          value={body}
          readOnly={send.isPending}
          aria-invalid={error !== null}
          aria-describedby={error ? `${id}-error` : undefined}
          onKeyDown={handleKeyDown}
          onChange={(event) => {
            setBody(event.target.value);
            setValidationError(null);
          }}
        />

        <Button type="submit" loading={send.isPending}>
          Send
        </Button>
      </div>

      {error && (
        <span id={`${id}-error`} role="alert" className={styles.error}>
          {error}
        </span>
      )}
    </form>
  );
};
