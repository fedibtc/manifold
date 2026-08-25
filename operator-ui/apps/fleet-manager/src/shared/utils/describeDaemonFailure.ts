import { ADMIN_ROUTE } from '@/shared/api/adminCall';
import {
  AccessDeniedError,
  AuthError,
  HttpStatusError,
  NetworkError,
  ProtocolError
} from '@/shared/api/errors';

// The technical detail line on the boot error screen. It states what was
// observed and nothing more: the class of failure and, where one exists, the
// status the daemon answered with.
//
// Error messages are never interpolated here. A message can carry a URL, a
// header or a fragment of a body, and this line renders on a screen no one has
// signed in to yet.
const observe = (error: unknown): string => {
  if (error instanceof NetworkError) return 'no response — the connection failed';
  if (error instanceof AuthError) return 'HTTP 401';
  if (error instanceof AccessDeniedError) return 'HTTP 403';
  if (error instanceof HttpStatusError) return `HTTP ${error.status}`;
  if (error instanceof ProtocolError) return 'answered, but not with an admin result';
  return 'failed for an unrecognised reason';
};

export const describeDaemonFailure = (error: unknown): string =>
  `POST ${ADMIN_ROUTE} · ${observe(error)}`;

export type DaemonFailureKind = 'refused' | 'unreachable';

// The two things the boot error screen can truthfully say. A 403 is the daemon
// answering about the request: it is up, it read the request, and it will answer
// the same way until something changes on its side — so the screen must not send
// the operator to restart a service that is already running. Everything else it
// is shown for means the dashboard is not being served at all.
export const daemonFailureKind = (error: unknown): DaemonFailureKind =>
  error instanceof AccessDeniedError ? 'refused' : 'unreachable';
