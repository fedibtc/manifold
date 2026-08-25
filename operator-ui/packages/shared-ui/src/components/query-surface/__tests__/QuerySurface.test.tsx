import { fireEvent, render, screen } from '@testing-library/react';
import { vi } from 'vitest';
import { QuerySurface } from '../QuerySurface';

const CONTENT = 'Earned, all time';
// Stands in for an app's describeActionError. The surface renders whatever
// sentence the app's error vocabulary produces; it does not own one.
const describeError = (error: unknown) => (error instanceof Error ? error.message : 'unknown');

it('should show only a loading line while nothing has answered', () => {
  render(
    <QuerySurface
      disposition={{ kind: 'loading' }}
      onRetry={() => {}}
      describeError={describeError}
    >
      <p>{CONTENT}</p>
    </QuerySurface>
  );

  screen.getByText('Loading…');
  expect(screen.queryByText(CONTENT)).not.toBeInTheDocument();
});

it('should show the failure and a retry control when nothing has answered', () => {
  render(
    <QuerySurface
      disposition={{ kind: 'failed', error: new Error('seats unavailable') }}
      onRetry={() => {}}
      describeError={describeError}
    >
      <p>{CONTENT}</p>
    </QuerySurface>
  );

  screen.getByText('seats unavailable');
  screen.getByRole('button', { name: 'Try again' });
  expect(screen.queryByText(CONTENT)).not.toBeInTheDocument();
});

it('should force an attempt when the retry control is used', () => {
  const onRetry = vi.fn();
  render(
    <QuerySurface
      disposition={{ kind: 'failed', error: new Error('down') }}
      onRetry={onRetry}
      describeError={describeError}
    >
      <p>{CONTENT}</p>
    </QuerySurface>
  );

  fireEvent.click(screen.getByRole('button', { name: 'Try again' }));

  expect(onRetry).toHaveBeenCalledTimes(1);
});

it('should keep the content under a staleness marker when a refresh failed', () => {
  render(
    <QuerySurface disposition={{ kind: 'stale' }} onRetry={() => {}} describeError={describeError}>
      <p>{CONTENT}</p>
    </QuerySurface>
  );

  screen.getByText('Showing last-known data');
  screen.getByText(CONTENT);
});

it('should show the content alone once every read has answered', () => {
  render(
    <QuerySurface
      disposition={{ kind: 'content' }}
      onRetry={() => {}}
      describeError={describeError}
    >
      <p>{CONTENT}</p>
    </QuerySurface>
  );

  screen.getByText(CONTENT);
  expect(screen.queryByText('Showing last-known data')).not.toBeInTheDocument();
  expect(screen.queryByText('Loading…')).not.toBeInTheDocument();
});
