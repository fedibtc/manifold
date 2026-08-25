import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render as rtlRender, screen } from '@testing-library/react';
import type { ReactElement } from 'react';
import { describe, expect, it } from 'vitest';
import { SetupWizard } from '@/features/setup/components/setup-wizard/SetupWizard';

// The gateway step reads the gateway's identity through a mutation, so mounting
// the wizard needs a client.
const render = (ui: ReactElement) =>
  rtlRender(
    <QueryClientProvider
      client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
    >
      {ui}
    </QueryClientProvider>
  );

const heading = () => screen.getByRole('heading', { level: 1 }).textContent;

const trustPanel = <p>Trust panel stub</p>;

describe('SetupWizard', () => {
  it('should advance to the Gateway step on Continue when Network is valid, then go Back', () => {
    render(<SetupWizard trustPanel={trustPanel} onComplete={() => {}} />);
    expect(heading()).toBe('Setup — Network');

    fireEvent.click(screen.getByText('Continue'));
    expect(heading()).toBe('Setup — Gateway');

    fireEvent.click(screen.getByText('Back'));
    expect(heading()).toBe('Setup — Network');
  });

  it('should hide the Back button on the first step', () => {
    render(<SetupWizard trustPanel={trustPanel} onComplete={() => {}} />);
    expect(screen.queryByText('Back')).toBeNull();
  });
});
