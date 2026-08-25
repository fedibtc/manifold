import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { AuthPromptPage } from '../AuthPromptPage';

vi.mock('@/shared/api/tokenStore', () => ({
  setToken: vi.fn()
}));

import { setToken } from '@/shared/api/tokenStore';

const wrapper = (children: ReactNode) => {
  const client = new QueryClient();
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
};

const submitToken = (token: string) => {
  const input = screen.getByLabelText('Admin token') as HTMLInputElement;
  fireEvent.change(input, { target: { value: token } });
  fireEvent.submit(input.closest('form') as HTMLFormElement);
};

// jsdom's Storage stub is not functional here, so install fakes we can assert on.
const fakeStorage = () => ({ setItem: vi.fn(), getItem: vi.fn(), length: 0 });
const originalLocal = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
const originalSession = Object.getOwnPropertyDescriptor(globalThis, 'sessionStorage');
let localSpy = fakeStorage();
let sessionSpy = fakeStorage();

describe('AuthPromptPage', () => {
  beforeEach(() => {
    localSpy = fakeStorage();
    sessionSpy = fakeStorage();
    Object.defineProperty(globalThis, 'localStorage', { value: localSpy, configurable: true });
    Object.defineProperty(globalThis, 'sessionStorage', { value: sessionSpy, configurable: true });
  });

  afterEach(() => {
    vi.clearAllMocks();
    if (originalLocal) Object.defineProperty(globalThis, 'localStorage', originalLocal);
    if (originalSession) Object.defineProperty(globalThis, 'sessionStorage', originalSession);
  });

  it('should call setToken with the submitted value', () => {
    render(wrapper(<AuthPromptPage />));

    submitToken('super-secret');

    expect(setToken).toHaveBeenCalledWith('super-secret');
  });

  it('should not write the token to localStorage or sessionStorage', () => {
    render(wrapper(<AuthPromptPage />));

    submitToken('super-secret');

    expect(localSpy.setItem).not.toHaveBeenCalled();
    expect(sessionSpy.setItem).not.toHaveBeenCalled();
  });

  it('should render a password input so the token is never echoed', () => {
    render(wrapper(<AuthPromptPage />));

    const input = screen.getByLabelText('Admin token') as HTMLInputElement;
    expect(input.type).toBe('password');
  });
});
