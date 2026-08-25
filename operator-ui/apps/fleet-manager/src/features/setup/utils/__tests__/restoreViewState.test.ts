import { describe, expect, it } from 'vitest';
import { classifyRestoreError } from '@/features/setup/utils/restoreViewState';
import { AdminApiError, AuthError, NetworkError } from '@/shared/api/errors';

describe('classifyRestoreError', () => {
  it('should carry a daemon refusal through unchanged', () => {
    const error = new AdminApiError('seat directory already exists: /var/lib/fman/seats/abc');

    expect(classifyRestoreError(error)).toEqual({
      errorClass: 'daemon',
      reason: 'other',
      message: 'seat directory already exists: /var/lib/fman/seats/abc'
    });
  });

  it('should classify an authentication refusal', () => {
    expect(classifyRestoreError(new AuthError())).toEqual({
      errorClass: 'auth',
      message: 'Your session expired. Sign in again.'
    });
  });

  it('should classify a transport failure', () => {
    expect(classifyRestoreError(new NetworkError())).toEqual({
      errorClass: 'network',
      message: 'The connection to the fleet manager failed before it answered.'
    });
  });

  it('should treat an unrecognised throw as a transport failure', () => {
    // A thrown value with no error class could be anything, including a response
    // the daemon acted on. Unknown is the only safe reading.
    expect(classifyRestoreError('boom')).toEqual({
      errorClass: 'network',
      message: 'The connection to the fleet manager failed before it answered.'
    });
  });

  it('should never invite a retry in an unknown result', () => {
    // `describeActionError` ends every transport failure with "Try again". A
    // second restore cannot be undone, so this class must not carry that word.
    expect(classifyRestoreError(new NetworkError()).message).not.toMatch(/try again/i);
    expect(classifyRestoreError('boom').message).not.toMatch(/try again/i);
  });
});
