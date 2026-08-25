import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, vi } from 'vitest';
import * as authenticateModule from '@/shared/api/authenticate';
import { InvalidPasswordError } from '@/shared/api/authenticate';
import { AuthPromptPage } from '../AuthPromptPage';

const renderPage = () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <AuthPromptPage />
    </QueryClientProvider>
  );
};

afterEach(() => {
  vi.restoreAllMocks();
});

it('should call authenticate with the entered password on submit', async () => {
  const authenticateSpy = vi.spyOn(authenticateModule, 'authenticate').mockResolvedValue(undefined);
  renderPage();

  fireEvent.change(screen.getByLabelText('Password'), { target: { value: 'test-password' } });
  fireEvent.click(screen.getByRole('button', { name: 'Sign in' }));

  await waitFor(() => expect(authenticateSpy).toHaveBeenCalledWith('test-password'));
});

it('should show an inline error and keep the field editable on a wrong password', async () => {
  vi.spyOn(authenticateModule, 'authenticate').mockRejectedValue(new InvalidPasswordError());
  renderPage();

  const input = screen.getByLabelText('Password') as HTMLInputElement;
  fireEvent.change(input, { target: { value: 'wrong' } });
  fireEvent.click(screen.getByRole('button', { name: 'Sign in' }));

  await waitFor(() => screen.getByText(/incorrect password/i));
  expect(input.disabled).toBe(false);
});
