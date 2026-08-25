import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render as rtlRender, screen, waitFor } from '@testing-library/react';
import type { ReactElement } from 'react';
import { useState } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { GatewayStep } from '@/features/setup/components/steps/gateway-step/GatewayStep';
import { type ConfigDraft, initialDraft } from '@/features/setup/services/draft';
import { validateGateway } from '@/features/setup/services/validation';
import * as adminCallModule from '@/shared/api/adminCall';

// The step reads the gateway's identity through a mutation, so it needs a
// client even in the cases that never fire the probe.
const render = (ui: ReactElement) =>
  rtlRender(
    <QueryClientProvider
      client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
    >
      {ui}
    </QueryClientProvider>
  );

const Harness = ({
  admin_url = '',
  credential = ''
}: {
  admin_url?: string;
  credential?: string;
}) => {
  const [draft, setDraft] = useState<ConfigDraft>({
    ...initialDraft,
    gateway: { ...initialDraft.gateway, admin_url },
    secrets: { ...initialDraft.secrets, gatewayAdminCredential: credential }
  });
  const onChange = (patch: Partial<ConfigDraft>) => setDraft((d) => ({ ...d, ...patch }));
  return <GatewayStep draft={draft} onChange={onChange} errors={{}} />;
};

describe('GatewayStep', () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('should update draft.gateway.admin_url on input', () => {
    render(<Harness />);
    const input = screen.getByLabelText('Admin URL') as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'https://gw.example.com' } });
    expect(input.value).toBe('https://gw.example.com');
  });

  it('should surface an error for a non-URL admin_url on validate', () => {
    const draft: ConfigDraft = {
      ...initialDraft,
      gateway: {
        ...initialDraft.gateway,
        gateway_name: 'gw',
        admin_url: 'not a url'
      }
    };
    const errors = validateGateway(draft);
    render(<GatewayStep draft={draft} onChange={() => {}} errors={errors} />);
    expect(screen.getByText('Enter a valid URL.')).toBeTruthy();
  });

  // The identity is frozen at first setup and decides which gateway an accepted
  // allocation pays, so the operator never types it. Until it has been read
  // there is nothing to continue with.
  it('should ask for the identity to be read rather than typed', () => {
    const draft: ConfigDraft = {
      ...initialDraft,
      gateway: { ...initialDraft.gateway, gateway_name: 'gw', admin_url: 'https://gw.example.com' },
      secrets: { ...initialDraft.secrets, gatewayAdminCredential: 'secret' }
    };

    render(<GatewayStep draft={draft} onChange={() => {}} errors={validateGateway(draft)} />);

    expect(screen.getByRole('button', { name: 'Connect to gateway' })).toBeTruthy();
    expect(screen.queryByLabelText(/gateway id/i)).toBeNull();
    expect(validateGateway(draft).gateway_id).toBeTruthy();
  });

  // The whole point of the change: the identifier is fetched, not transcribed.
  // The credential is stored first, because the daemon authenticates the probe
  // with the stored one rather than accepting a secret in the request.
  it('should store the credential, then read the identity from the gateway', async () => {
    const adminCall = vi
      .spyOn(adminCallModule, 'adminCall')
      .mockResolvedValueOnce({ secret: 'gateway_admin_credential', present: true } as never)
      .mockResolvedValueOnce({
        gateway_id: '02abc',
        network: 'signet',
        lightning_alias: 'my-gateway'
      } as never);

    render(<Harness admin_url="https://gw.example.com" credential="secret" />);

    fireEvent.click(screen.getByRole('button', { name: 'Connect to gateway' }));

    await waitFor(() =>
      expect(adminCall).toHaveBeenNthCalledWith(2, 'probe_gateway', {
        admin_url: 'https://gw.example.com'
      })
    );
    expect(await screen.findByText('Connected to my-gateway')).toBeTruthy();
    // Fetched into the draft, so the review step and the daemon see the same id.
    await waitFor(() => expect(screen.getByRole('button', { name: 'Check again' })).toBeTruthy());
  });
});
