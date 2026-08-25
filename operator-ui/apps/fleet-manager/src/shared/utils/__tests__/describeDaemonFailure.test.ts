import {
  AccessDeniedError,
  AuthError,
  HttpStatusError,
  NetworkError,
  ProtocolError
} from '@/shared/api/errors';
import { daemonFailureKind, describeDaemonFailure } from '../describeDaemonFailure';

it('should name the route and the transport failure when nothing answered', () => {
  expect(describeDaemonFailure(new NetworkError())).toBe(
    'POST /api/admin · no response — the connection failed'
  );
});

it('should state the observed status instead of guessing at a refused connection', () => {
  expect(describeDaemonFailure(new HttpStatusError(503))).toBe('POST /api/admin · HTTP 503');
});

it('should state a 403 as a status, never as an unreachable daemon', () => {
  expect(describeDaemonFailure(new AccessDeniedError())).toBe('POST /api/admin · HTTP 403');
});

it('should state a 401 as a status', () => {
  expect(describeDaemonFailure(new AuthError())).toBe('POST /api/admin · HTTP 401');
});

it('should say the answer was not an admin result on a protocol failure', () => {
  expect(describeDaemonFailure(new ProtocolError())).toBe(
    'POST /api/admin · answered, but not with an admin result'
  );
});

it('should not invent a cause for an unrecognised failure', () => {
  expect(describeDaemonFailure(null)).toBe('POST /api/admin · failed for an unrecognised reason');
});

it('should read a 403 as refused, since the daemon answered', () => {
  expect(daemonFailureKind(new AccessDeniedError())).toBe('refused');
});

it('should read a transport failure, a server error and a bad answer as unreachable', () => {
  expect(daemonFailureKind(new NetworkError())).toBe('unreachable');
  expect(daemonFailureKind(new HttpStatusError(502))).toBe('unreachable');
  expect(daemonFailureKind(new ProtocolError())).toBe('unreachable');
});

it('should never render the error message, which can carry request detail', () => {
  const detail = describeDaemonFailure(new NetworkError('fetch http://fman.internal/api/admin'));

  expect(detail).not.toMatch(/fman.internal/);
});
