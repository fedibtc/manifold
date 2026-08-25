import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, renderHook } from '@testing-library/react';
import type { ChangeEvent, FormEvent, ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as authenticateModule from '@/shared/api/authenticate';
import { InvalidPasswordError } from '@/shared/api/authenticate';
import { HttpStatusError, NetworkError } from '@/shared/api/errors';
import { useAuthPrompt } from '../useAuthPrompt';

const wrapper = ({ children }: { children: ReactNode }) => (
  <QueryClientProvider client={new QueryClient()}>{children}</QueryClientProvider>
);

const changeEvent = (value: string) => ({ target: { value } }) as ChangeEvent<HTMLInputElement>;
const submitEvent = { preventDefault: vi.fn() } as unknown as FormEvent<HTMLFormElement>;

describe('useAuthPrompt', () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('should authenticate with the entered password and clear the field', async () => {
    const authenticateSpy = vi
      .spyOn(authenticateModule, 'authenticate')
      .mockResolvedValue(undefined);
    const { result } = renderHook(() => useAuthPrompt(), { wrapper });

    act(() => result.current.onPasswordChange(changeEvent('test-password')));
    await act(async () => {
      await result.current.onSubmit(submitEvent);
    });

    expect(authenticateSpy).toHaveBeenCalledWith('test-password');
    expect(result.current.password).toBe('');
    expect(result.current.error).toBeNull();
  });

  it('should surface an inline error on a wrong password', async () => {
    vi.spyOn(authenticateModule, 'authenticate').mockRejectedValue(new InvalidPasswordError());
    const { result } = renderHook(() => useAuthPrompt(), { wrapper });

    act(() => result.current.onPasswordChange(changeEvent('wrong')));
    await act(async () => {
      await result.current.onSubmit(submitEvent);
    });

    expect(result.current.error).toBe('Incorrect password. Try again.');
    expect(result.current.isSubmitting).toBe(false);
  });

  it('should not blame the password when the fleet manager could not be reached', async () => {
    vi.spyOn(authenticateModule, 'authenticate').mockRejectedValue(new NetworkError());
    const { result } = renderHook(() => useAuthPrompt(), { wrapper });

    act(() => result.current.onPasswordChange(changeEvent('correct-password')));
    await act(async () => {
      await result.current.onSubmit(submitEvent);
    });

    expect(result.current.error).toMatch(/can't reach the fleet manager/i);
    expect(result.current.error).not.toMatch(/incorrect password/i);
  });

  // Three separate facts, three separate lines: a server error is not a wrong
  // password, not an unreachable daemon, and not something this side can say the
  // password went unread through — so it states the status and stops there.
  it('should state the status without blaming the password when the fleet manager answered with a server error', async () => {
    vi.spyOn(authenticateModule, 'authenticate').mockRejectedValue(new HttpStatusError(500));
    const { result } = renderHook(() => useAuthPrompt(), { wrapper });

    act(() => result.current.onPasswordChange(changeEvent('correct-password')));
    await act(async () => {
      await result.current.onSubmit(submitEvent);
    });

    expect(result.current.error).toBe(
      'The fleet manager failed while signing in (HTTP 500). That is a fault in the service, not a wrong password. Check the service, then try again.'
    );
  });
});
