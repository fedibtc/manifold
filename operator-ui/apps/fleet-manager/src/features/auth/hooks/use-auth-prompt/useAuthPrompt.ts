import { useQueryClient } from '@tanstack/react-query';
import { type ChangeEvent, type FormEvent, useState } from 'react';
import { describeAuthFailure } from '@/features/auth/utils/describeAuthFailure';
import { authenticate } from '@/shared/api/authenticate';
import { ONBOARDING_KEY } from '@/shared/api/hooks/use-onboarding/useOnboarding';

export interface AuthPrompt {
  password: string;
  error: string | null;
  isSubmitting: boolean;
  onPasswordChange: (event: ChangeEvent<HTMLInputElement>) => void;
  onSubmit: (event: FormEvent<HTMLFormElement>) => Promise<void>;
}

// Password sign-in (US-FMAN-001). No admin call fires before POST /api/auth
// succeeds; the session lives in an HttpOnly cookie the browser manages, so there
// is nothing here to store or paste — only a password to submit.
export const useAuthPrompt = (): AuthPrompt => {
  const queryClient = useQueryClient();
  const [password, setPassword] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [isSubmitting, setIsSubmitting] = useState(false);

  const onPasswordChange = (event: ChangeEvent<HTMLInputElement>) => {
    setPassword(event.target.value);
  };

  const onSubmit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!password || isSubmitting) return;

    setIsSubmitting(true);
    setError(null);
    try {
      await authenticate(password);
      setPassword('');
      await queryClient.invalidateQueries({ queryKey: ONBOARDING_KEY });
    } catch (failure) {
      setError(describeAuthFailure(failure));
    } finally {
      setIsSubmitting(false);
    }
  };

  return { password, error, isSubmitting, onPasswordChange, onSubmit };
};
