import type { ServiceError, ServiceErrorCode } from '@operator-ui/types';
import { HttpResponse, http, type JsonBodyType, type RequestHandler } from 'msw';
import { getState } from '@/mocks/state';
import { mockStore } from '@/mocks/store';
import { verbLog } from '@/mocks/verb-log';
import { withRestoreMarker } from '@/mocks/world/health';
import { dispatch, isServiceErrorLike, MUTATING_VERBS } from '@/mocks/world/verbs';

// permission_denied → 403 (an authenticated request denied by policy), per
// SPEC-flip-admin-api.md:31-33 and admin_http.rs's status_for_error. Missing
// or invalid bearer is a separate, always-401 case handled below — it never
// goes through this table.
const HTTP_STATUS: Record<ServiceErrorCode, number> = {
  invalid_argument: 400,
  failed_precondition: 400,
  permission_denied: 403,
  not_found: 404,
  unavailable: 503,
  internal: 500,
  unknown: 500
};

const serviceError = (code: ServiceErrorCode, message: string) =>
  HttpResponse.json({ code, message } satisfies ServiceError, { status: HTTP_STATUS[code] });

const delay = async (): Promise<void> => {
  const { latencyMs } = getState();
  if (latencyMs > 0) await new Promise((resolve) => setTimeout(resolve, latencyMs));
};

export const handlers: RequestHandler[] = [
  // Unauthenticated liveness probe. The SPA boot sequence reads
  // health.components before the operator has authenticated, so this must
  // serve the full GetHealthResponse, not a bare {status}.
  http.get('*/health', () => {
    const { health, bootMode } = getState();
    return HttpResponse.json(withRestoreMarker(health, bootMode));
  }),

  http.post('*/admin/v1/:method', async ({ request, params }) => {
    const method = String(params.method);

    const header = request.headers.get('authorization') ?? '';
    const token = header.startsWith('Bearer ') ? header.slice(7).trim() : '';
    // Missing/invalid bearer is always 401, regardless of the permission_denied
    // ServiceError code it carries — matches admin_http.rs's require_auth_token,
    // which fixes this case to StatusCode::UNAUTHORIZED ahead of status_for_error.
    if (!token) {
      return HttpResponse.json(
        { code: 'permission_denied', message: 'missing bearer token' } satisfies ServiceError,
        { status: 401 }
      );
    }

    // Recorded before the error checks below, so a verb the panel has just
    // forced into failing stays listed and can be switched back.
    verbLog.record(method);

    const injected = new URL(request.url).searchParams.get('error');
    if (injected) {
      const code = injected === '503' ? 'unavailable' : (injected as ServiceErrorCode);
      return serviceError(code, 'route not available in mock');
    }

    const forced = getState().forcedErrors[method];
    if (forced) {
      const code = forced === '503' ? 'unavailable' : forced;
      return serviceError(code, 'route not available in mock');
    }

    await delay();

    try {
      // Every verb returns a plain response object; dispatch's signature stays
      // `unknown` (pinned in world/verbs.ts) so this cast reflects the actual
      // runtime shape rather than widening it.
      const result = dispatch(method, await request.json()) as JsonBodyType;
      if (MUTATING_VERBS.has(method)) mockStore.persist();
      return HttpResponse.json(result);
    } catch (error) {
      if (isServiceErrorLike(error)) return serviceError(error.code, error.message);
      throw error;
    }
  })
];
