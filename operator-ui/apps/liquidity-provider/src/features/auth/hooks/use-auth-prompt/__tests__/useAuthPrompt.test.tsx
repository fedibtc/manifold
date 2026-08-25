import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, renderHook } from '@testing-library/react';
import type { ChangeEvent, FormEvent, ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';

vi.mock('@/shared/api/tokenStore', () => ({ setToken: vi.fn() }));

import { setToken } from '@/shared/api/tokenStore';
import { useAuthPrompt } from '../useAuthPrompt';

const wrapper = ({ children }: { children: ReactNode }) => (
  <QueryClientProvider client={new QueryClient()}>{children}</QueryClientProvider>
);

const changeEvent = (value: string) => ({ target: { value } }) as ChangeEvent<HTMLInputElement>;
const submitEvent = { preventDefault: vi.fn() } as unknown as FormEvent<HTMLFormElement>;

describe('useAuthPrompt', () => {
  afterEach(() => {
    vi.clearAllMocks();
  });

  it('should store the submitted token and clear the field', () => {
    const { result } = renderHook(() => useAuthPrompt(), { wrapper });

    act(() => result.current.onChange(changeEvent('super-secret')));
    act(() => result.current.onSubmit(submitEvent));

    expect(setToken).toHaveBeenCalledWith('super-secret');
    expect(result.current.value).toBe('');
  });

  it('should ignore an empty submit', () => {
    const { result } = renderHook(() => useAuthPrompt(), { wrapper });

    act(() => result.current.onSubmit(submitEvent));

    expect(setToken).not.toHaveBeenCalled();
  });
});
