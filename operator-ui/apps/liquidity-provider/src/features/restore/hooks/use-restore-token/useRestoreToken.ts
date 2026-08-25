import { type ChangeEvent, type FormEvent, useState } from 'react';
import { getToken, setToken } from '@/shared/api/tokenStore';

export interface RestoreToken {
  tokenEntered: boolean;
  tokenValue: string;
  onTokenChange: (event: ChangeEvent<HTMLInputElement>) => void;
  onTokenSubmit: (event: FormEvent<HTMLFormElement>) => void;
}

// Operator-token gate for the restore console. inspect_backup/restore_backup are
// the most destructive calls in the app and the restore-mode boot path skips the
// normal auth gate, so the token (in-memory only) must be entered before the
// archive controls become reachable.
export const useRestoreToken = (): RestoreToken => {
  const [tokenEntered, setTokenEntered] = useState(() => Boolean(getToken()));
  const [tokenValue, setTokenValue] = useState('');

  const onTokenChange = (event: ChangeEvent<HTMLInputElement>) => {
    setTokenValue(event.target.value);
  };

  const onTokenSubmit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!tokenValue) return;
    setToken(tokenValue);
    setTokenValue('');
    setTokenEntered(true);
  };

  return { tokenEntered, tokenValue, onTokenChange, onTokenSubmit };
};
