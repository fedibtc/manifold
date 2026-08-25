import { useQueryClient } from '@tanstack/react-query';
import { type ChangeEvent, type FormEvent, useState } from 'react';
import { HEALTH_KEY } from '@/shared/api/hooks/use-health/useHealth';
import { SETUP_STATE_KEY } from '@/shared/api/hooks/use-setup-state/useSetupState';
import { setToken } from '@/shared/api/tokenStore';

export interface AuthPrompt {
  value: string;
  onChange: (event: ChangeEvent<HTMLInputElement>) => void;
  onSubmit: (event: FormEvent<HTMLFormElement>) => void;
}

// Operator token prompt. The token lives only in the in-memory tokenStore —
// never persisted, never logged — and its submit refreshes the gated queries.
export const useAuthPrompt = (): AuthPrompt => {
  const queryClient = useQueryClient();
  const [value, setValue] = useState('');

  const onChange = (event: ChangeEvent<HTMLInputElement>) => {
    setValue(event.target.value);
  };

  const onSubmit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!value) return;

    setToken(value);
    setValue('');
    void queryClient.invalidateQueries({ queryKey: SETUP_STATE_KEY });
    void queryClient.invalidateQueries({ queryKey: HEALTH_KEY });
  };

  return { value, onChange, onSubmit };
};
