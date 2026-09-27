import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import fixture from '../../../../../packages/types/fixtures/fman_self_check.json';
import { SelfCheckCard } from './SelfCheckCard';

const call = vi.hoisted(() => vi.fn());
vi.mock('@/shared/api/adminCall', () => ({ adminCall: call }));

const renderCard = () =>
  render(
    <QueryClientProvider client={new QueryClient()}>
      <SelfCheckCard />
    </QueryClientProvider>
  );

beforeEach(() => call.mockReset());

it('runs only on click, previews and copies exactly the report', async () => {
  call.mockResolvedValue(fixture);
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });
  renderCard();
  expect(call).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Run self-check' }));
  await screen.findByLabelText('Report preview');
  expect(call).toHaveBeenCalledWith('RunSelfCheck');
  const value = (screen.getByLabelText('Report preview') as HTMLTextAreaElement).value;
  fireEvent.click(screen.getByRole('button', { name: 'Copy report' }));
  await waitFor(() => expect(writeText).toHaveBeenCalledWith(value));
});

it('hides the old report on a rejected rerun and does not copy transport errors', async () => {
  call.mockResolvedValueOnce(fixture).mockRejectedValueOnce(new Error('private-host secret'));
  renderCard();
  fireEvent.click(screen.getByRole('button', { name: 'Run self-check' }));
  await screen.findByLabelText('Report preview');
  fireEvent.click(screen.getByRole('button', { name: 'Run self-check' }));
  await screen.findByText(/request failed/);
  expect(screen.queryByLabelText('Report preview')).toBeNull();
  expect(screen.queryByText(/private-host/)).toBeNull();
});

it('offers a manual copy of the same preview if clipboard permission fails', async () => {
  call.mockResolvedValue(fixture);
  Object.defineProperty(navigator, 'clipboard', {
    configurable: true,
    value: {
      writeText: vi.fn().mockRejectedValue(new Error('private-host secret'))
    }
  });
  renderCard();
  fireEvent.click(screen.getByRole('button', { name: 'Run self-check' }));
  await screen.findByLabelText('Report preview');
  fireEvent.click(screen.getByRole('button', { name: 'Copy report' }));
  await screen.findByText('Select and copy the report manually.');
  expect(screen.queryByText(/private-host/)).toBeNull();
});
