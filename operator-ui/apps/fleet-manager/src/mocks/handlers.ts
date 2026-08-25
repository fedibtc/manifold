import { HttpResponse, http, type RequestHandler } from 'msw';
import { getState } from '@/mocks/state';
import { mockStore } from '@/mocks/store';
import { verbLog } from '@/mocks/verb-log';
import { dispatch, MUTATING_VERBS, parseRequest } from '@/mocks/world/verbs';

// Mirrors crates/fman/core/src/admin_http.rs. The real adapter names the cookie
// randomly per process; a fixed name is fine for a dev mock and easier to debug.
const SESSION_COOKIE_NAME = 'fman_mock_session';
const SESSION_COOKIE_VALUE = 'mock-session-token';

const delay = async (): Promise<void> => {
  const { latencyMs } = getState();
  if (latencyMs > 0) await new Promise((resolve) => setTimeout(resolve, latencyMs));
};

// Real behavior (fedimint_ui_common::auth::require_auth): trusted-proxy mode
// does no local auth; password mode answers a bare 401 with no JSON body.
const isAuthorized = (): boolean => {
  const { authMode, sessionActive } = getState();
  return authMode === 'trusted_proxy' || sessionActive;
};

export const handlers: RequestHandler[] = [
  // Trusted-proxy mode mounts no /api/auth route at all in the real adapter.
  http.post('*/api/auth', async ({ request }) => {
    const state = getState();
    if (state.authMode === 'trusted_proxy') return new HttpResponse(null, { status: 404 });

    const { password } = (await request.json()) as { password?: string };
    if (password !== state.password) return new HttpResponse(null, { status: 401 });

    state.sessionActive = true;
    mockStore.persist();
    // Emitted for realism; the persisted flag above is the source of truth,
    // because a browser cannot hold an HttpOnly cookie the way the daemon sets one.
    return new HttpResponse(null, {
      status: 204,
      headers: {
        'set-cookie': `${SESSION_COOKIE_NAME}=${SESSION_COOKIE_VALUE}; HttpOnly; SameSite=Lax; Path=/`
      }
    });
  }),

  // One route, dispatched on the body: AdminRequest is externally tagged, so a
  // unit variant is a bare string and a struct variant a single-key object.
  http.post('*/api/admin', async ({ request }) => {
    const body = await request.json();
    const { method } = parseRequest(body);
    const state = getState();

    // Consumed before the auth check, so the tester's next submit is the one that
    // is refused rather than some later poll.
    if (method === 'OnboardFromBackup' && state.restoreSession === 'expire-on-submit') {
      state.sessionActive = false;
      state.restoreSession = 'active';
      mockStore.persist();
      return new HttpResponse(null, { status: 401 });
    }

    if (!isAuthorized()) return new HttpResponse(null, { status: 401 });

    await delay();

    // A transport failure is not a daemon refusal. HttpResponse.error() is what
    // makes `adminCall` raise NetworkError, which is what the unknown-result screen
    // keys on — a 500 or a JSON body would produce an AdminApiError instead.
    if (method === 'Onboarding' && state.onboardingTransport === 'network-failure') {
      return HttpResponse.error();
    }

    if (method === 'OnboardFromBackup' && state.restoreTransport === 'fail-before-dispatch') {
      return HttpResponse.error();
    }

    const result = dispatch(body);

    // Feeds the dev panel's per-page tab, which lists what a page actually
    // calls rather than what a hand-written map claims it calls.
    verbLog.record(method);
    if (MUTATING_VERBS.has(method)) mockStore.persist();

    // The daemon acted and the browser never heard. This is the state that
    // BE-FMAN-RECOVERY-002 exists to make recoverable.
    if (method === 'OnboardFromBackup' && state.restoreTransport === 'fail-after-commit') {
      return HttpResponse.error();
    }

    return HttpResponse.json(result, { headers: { 'cache-control': 'no-store' } });
  })
];
