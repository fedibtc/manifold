import type { SeatGrantEntry } from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { GuardianCodeCreate } from '../GuardianCodeCreate';

const TRUST =
  'I trust the Federation Ambassador I share this code with to choose the other guardians.';

const created: SeatGrantEntry = {
  grant_nonce: 'ab'.repeat(32),
  label: null,
  issued_at: 1_786_000_000,
  status: { state: 'unused', uri: 'manifold:seat-grant?v=1' }
};

const renderCreate = () => {
  const onCreated = vi.fn();
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <GuardianCodeCreate onCreated={onCreated} onCancel={vi.fn()} />
    </QueryClientProvider>
  );
  return { onCreated };
};

const createButton = () =>
  screen.getByRole('button', { name: 'Create guardian code' }) as HTMLButtonElement;

afterEach(() => {
  vi.restoreAllMocks();
});

describe('GuardianCodeCreate', () => {
  it('should not create a code until the operator accepts the delegation', () => {
    renderCreate();

    expect(createButton().disabled).toBe(true);
    fireEvent.click(screen.getByLabelText(TRUST));
    expect(createButton().disabled).toBe(false);
  });

  // Codes are always free: there is no price or free-setup choice to make.
  it('should offer no price choice', () => {
    renderCreate();

    expect(screen.queryByRole('switch')).toBeNull();
    expect(screen.queryByLabelText(/price|fee/i)).toBeNull();
  });

  // The label travels inside the signed code: it is not private to the operator.
  it('should say the label is shown to the Federation Ambassador', () => {
    renderCreate();

    expect(
      screen.getByText(/Shown to the Federation Ambassador as this guardian's name/)
    ).toBeTruthy();
    expect(screen.queryByText(/Only you see/)).toBeNull();
  });

  it('should send a blank label as no label', async () => {
    const call = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(created);
    const { onCreated } = renderCreate();

    fireEvent.change(screen.getByLabelText('Label (optional)'), { target: { value: '   ' } });
    fireEvent.click(screen.getByLabelText(TRUST));
    fireEvent.click(createButton());

    await waitFor(() => expect(onCreated).toHaveBeenCalledTimes(1));
    expect(call).toHaveBeenCalledWith({ CreateSeatGrant: { label: null } });
  });

  it('should send the trimmed label', async () => {
    const call = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(created);
    renderCreate();

    fireEvent.change(screen.getByLabelText('Label (optional)'), {
      target: { value: ' Lisbon meetup ' }
    });
    fireEvent.click(screen.getByLabelText(TRUST));
    fireEvent.click(createButton());

    await waitFor(() =>
      expect(call).toHaveBeenCalledWith({ CreateSeatGrant: { label: 'Lisbon meetup' } })
    );
  });

  it('should block a label over 32 characters and accept exactly 32', () => {
    renderCreate();
    fireEvent.click(screen.getByLabelText(TRUST));

    fireEvent.change(screen.getByLabelText('Label (optional)'), {
      target: { value: 'a'.repeat(33) }
    });
    expect(screen.getByRole('alert').textContent).toMatch(/32 characters or fewer/);
    expect(createButton().disabled).toBe(true);

    fireEvent.change(screen.getByLabelText('Label (optional)'), {
      target: { value: 'a'.repeat(32) }
    });
    expect(screen.queryByRole('alert')).toBeNull();
    expect(createButton().disabled).toBe(false);
  });
});
