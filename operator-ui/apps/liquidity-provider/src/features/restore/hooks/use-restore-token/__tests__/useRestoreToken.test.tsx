import { act, renderHook } from '@testing-library/react';
import type { ChangeEvent, FormEvent } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { clearToken, getToken, setToken } from '@/shared/api/tokenStore';
import { useRestoreToken } from '../useRestoreToken';

const changeEvent = (value: string) => ({ target: { value } }) as ChangeEvent<HTMLInputElement>;
const submitEvent = { preventDefault: vi.fn() } as unknown as FormEvent<HTMLFormElement>;

describe('useRestoreToken', () => {
  afterEach(() => {
    clearToken();
    vi.clearAllMocks();
  });

  it('should start ungated when no token is present', () => {
    clearToken();

    const { result } = renderHook(() => useRestoreToken());

    expect(result.current.tokenEntered).toBe(false);
  });

  it('should start gated open when a token was already set', () => {
    setToken('existing');

    const { result } = renderHook(() => useRestoreToken());

    expect(result.current.tokenEntered).toBe(true);
  });

  it('should store the submitted token and open the gate', () => {
    clearToken();

    const { result } = renderHook(() => useRestoreToken());
    act(() => result.current.onTokenChange(changeEvent('op-token')));
    act(() => result.current.onTokenSubmit(submitEvent));

    expect(getToken()).toBe('op-token');
    expect(result.current.tokenEntered).toBe(true);
    expect(result.current.tokenValue).toBe('');
  });
});
