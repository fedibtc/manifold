import type { AttestationListResponse, AttestationPayloadInfo } from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { AttestationPanel } from '../AttestationPanel';

const holderPubkey = '02aabbccddeeff00112233445566778899aabbccddeeff001122334455667788';

const payloads: AttestationPayloadInfo[] = [
  {
    id: 'att-1',
    kind: 'holder_authorization',
    subject: { holder: holderPubkey },
    ingested_at: 1784634480, // 2026-07-21T11:48:00Z
    valid: true
  },
  {
    id: 'att-2',
    kind: 'issuer_credential',
    issuer: '03bb'.padEnd(66, '0'),
    subject: { issuer: '03bb'.padEnd(66, '0') },
    ingested_at: 1784538000, // 2026-07-20T09:00:00Z
    valid: false
  }
];

const listResponse: AttestationListResponse = { payloads };

const makeClient = () => new QueryClient({ defaultOptions: { queries: { retry: false } } });

const wrap = (client: QueryClient) => {
  const Wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  return Wrapper;
};

afterEach(() => {
  vi.restoreAllMocks();
});

describe('AttestationPanel', () => {
  it('should render loading, then a row per installed attestation', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(listResponse);

    render(<AttestationPanel />, { wrapper: wrap(makeClient()) });

    expect(screen.getByText('Loading attestations…')).toBeTruthy();

    await waitFor(() => expect(screen.getByText('Holder authorization')).toBeTruthy());
    expect(screen.getByText('Issuer credential')).toBeTruthy();
    expect(screen.getByText('Valid')).toBeTruthy();
    expect(screen.getByText('Invalid')).toBeTruthy();
    expect(screen.getByText('2026-07-21')).toBeTruthy();
  });

  it('should show an empty state when no attestations are installed', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({ payloads: [] });

    render(<AttestationPanel />, { wrapper: wrap(makeClient()) });

    await waitFor(() => expect(screen.getByText('No attestations installed yet.')).toBeTruthy());
  });

  it('should show an error banner when the list query fails', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockRejectedValue(new Error('boom'));

    render(<AttestationPanel />, { wrapper: wrap(makeClient()) });

    await waitFor(() => expect(screen.getByText("Couldn't load attestations")).toBeTruthy());
  });

  it('should call attestation_install when Install is clicked with a selected file', async () => {
    const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockImplementation((method) => {
      if (method === 'attestation_list') return Promise.resolve(listResponse);
      if (method === 'attestation_install') {
        return Promise.resolve({ id: 'att-new', kind: 'holder_authorization' });
      }
      return Promise.resolve({});
    });

    render(<AttestationPanel />, { wrapper: wrap(makeClient()) });
    await waitFor(() => expect(screen.getByText('Holder authorization')).toBeTruthy());

    const bytes = new Uint8Array([9, 8, 7]);
    const file = new File([bytes], 'cred.bin');
    Object.defineProperty(file, 'arrayBuffer', {
      value: async () => bytes.buffer
    });
    fireEvent.change(screen.getByLabelText('Attestation file'), { target: { files: [file] } });
    fireEvent.click(screen.getByRole('button', { name: 'Install' }));

    await waitFor(() =>
      expect(adminCallSpy).toHaveBeenCalledWith('attestation_install', {
        payload: [9, 8, 7]
      })
    );
  });

  it('should call attestation_remove by id when Remove is clicked', async () => {
    const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockImplementation((method) => {
      if (method === 'attestation_list') return Promise.resolve(listResponse);
      return Promise.resolve({});
    });

    render(<AttestationPanel />, { wrapper: wrap(makeClient()) });
    await waitFor(() => expect(screen.getByText('Holder authorization')).toBeTruthy());

    fireEvent.click(screen.getAllByRole('button', { name: 'Remove' })[0]);

    await waitFor(() =>
      expect(adminCallSpy).toHaveBeenCalledWith('attestation_remove', {
        target: { id: 'att-1' }
      })
    );
  });
});
