import {
  AccessDeniedError,
  AdminApiError,
  AuthError,
  HttpStatusError,
  NetworkError,
  ProtocolError
} from '@/shared/api/errors';
import { describeActionError } from '../describeActionError';

it('should describe a NetworkError as the fleet manager being unreachable', () => {
  expect(describeActionError(new NetworkError())).toBe(
    "Can't reach the fleet manager. Try again once it's back online."
  );
});

it('should describe a server-side status as the fleet manager being unreachable', () => {
  expect(describeActionError(new HttpStatusError(502))).toBe(
    "Can't reach the fleet manager. Try again once it's back online."
  );
});

it('should describe an unreadable answer as the fleet manager being unreachable', () => {
  expect(describeActionError(new ProtocolError())).toBe(
    "Can't reach the fleet manager. Try again once it's back online."
  );
});

it('should describe a 403 as a refusal, never as an unreachable fleet manager', () => {
  const message = describeActionError(new AccessDeniedError());

  expect(message).toMatch(/refused this request/i);
  expect(message).not.toMatch(/can't reach/i);
  expect(message).not.toMatch(/sign in/i);
});

it('should state the status when a client-side status is refused', () => {
  expect(describeActionError(new HttpStatusError(404))).toBe(
    'The fleet manager refused the request (HTTP 404).'
  );
});

it('should describe an AuthError as an expired session', () => {
  expect(describeActionError(new AuthError())).toBe('Your session expired. Sign in again.');
});

it('should describe an AdminApiError with its message', () => {
  expect(describeActionError(new AdminApiError('unknown seat'))).toBe('unknown seat');
});

it('should fall back to a generic message for a plain error', () => {
  expect(describeActionError(new Error('boom'))).toBe('boom');
});

it('should fall back to a generic message for a non-error value', () => {
  expect(describeActionError('not an error')).toBe('Something went wrong. Please try again.');
});
