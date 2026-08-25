import { describe, expect, it } from 'vitest';
import { AdminApiError, AuthError, NetworkError } from '@/shared/api/errors';
import { describeActionError } from '../describeActionError';

describe('describeActionError', () => {
  it('should describe a NetworkError as the daemon being unreachable', () => {
    expect(describeActionError(new NetworkError('HTTP 503'))).toBe(
      'The funds daemon is unreachable. Try again once it is back online.'
    );
  });

  it('should surface the service code and message for an AdminApiError', () => {
    expect(
      describeActionError(
        new AdminApiError('failed_precondition', 'insufficient spendable balance')
      )
    ).toBe('failed_precondition: insufficient spendable balance');
  });

  it('should fall back to the message of any other Error', () => {
    expect(describeActionError(new AuthError())).toBe('unauthorized');
  });

  it('should return a generic message for a non-Error value', () => {
    expect(describeActionError('boom')).toBe('Something went wrong. Please try again.');
  });
});
