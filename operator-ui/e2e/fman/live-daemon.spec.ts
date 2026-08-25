import { expect, test } from '@playwright/test';
import { signIn } from './support/auth';

// @live specs run only under E2E_TARGET=daemon against a real, defe-provisioned
// Fleet Manager (via `just test-e2e-ui-fman`). They never run in mock mode.
// There is no scenario to reset here — the real daemon is the source of truth,
// and a freshly provisioned one owns no seats.

test('@live should sign in against the real operator API and show an empty fleet', async ({
  page
}) => {
  await page.goto('/seats');
  await signIn(page);

  // Reaching the app shell proves the whole live chain: the Vite proxy hits the
  // real /api/auth, the daemon accepted the password defe generated and issued a
  // session cookie, and the gating Onboarding call then succeeded over
  // /api/admin against the genuine dispatcher.
  await expect(page.getByRole('heading', { name: 'Seats', level: 1 })).toBeVisible();

  // A freshly provisioned manager owns no seats, so the real empty state renders
  // from real backend data rather than a fixture.
  await expect(
    page.getByText('No seats yet. Seats are created by Federation Initiators')
  ).toBeVisible();
});
