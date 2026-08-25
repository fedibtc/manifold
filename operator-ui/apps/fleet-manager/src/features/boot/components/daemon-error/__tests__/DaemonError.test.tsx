import { fireEvent, render, screen } from '@testing-library/react';
import { vi } from 'vitest';
import { AccessDeniedError, HttpStatusError, NetworkError } from '@/shared/api/errors';
import { DaemonError } from '../DaemonError';

it('should state plainly that guardians are supervised and down with the daemon', () => {
  render(<DaemonError failure={new NetworkError()} onRetry={vi.fn()} />);

  screen.getByText(/guardians are supervised by this daemon and are down too/i);
});

it('should call onRetry when the retry button is clicked', () => {
  const onRetry = vi.fn();
  render(<DaemonError failure={new NetworkError()} onRetry={onRetry} />);

  fireEvent.click(screen.getByRole('button', { name: 'Retry' }));

  expect(onRetry).toHaveBeenCalled();
});

it('should report a transport failure as no response', () => {
  render(<DaemonError failure={new NetworkError()} onRetry={vi.fn()} />);

  screen.getByText('POST /api/admin · no response — the connection failed');
});

it('should report the observed status rather than a hard-coded refused connection', () => {
  render(<DaemonError failure={new HttpStatusError(503)} onRetry={vi.fn()} />);

  screen.getByText('POST /api/admin · HTTP 503');
  expect(screen.queryByText(/connection refused/i)).toBeNull();
});

// A 403 came from a daemon that is running. Telling the operator to go and check
// that the service is up would send them after a process that is already there,
// and asking them to sign in again would blame a session that was accepted.
it('should say the request was refused, not that the daemon is unreachable, on a 403', () => {
  render(<DaemonError failure={new AccessDeniedError()} onRetry={vi.fn()} />);

  screen.getByRole('heading', { name: 'The fleet manager refused this dashboard' });
  screen.getByText('POST /api/admin · HTTP 403');
  screen.getByText(/signing in again will not change the answer/i);
  expect(screen.queryByText(/can't reach the fleet manager/i)).toBeNull();
  expect(screen.queryByText(/check that the fleet-manager service is running/i)).toBeNull();
});

it('should keep the retry control on a refused dashboard', () => {
  const onRetry = vi.fn();
  render(<DaemonError failure={new AccessDeniedError()} onRetry={onRetry} />);

  fireEvent.click(screen.getByRole('button', { name: 'Retry' }));

  expect(onRetry).toHaveBeenCalled();
});
