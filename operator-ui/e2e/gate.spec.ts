import { expect, test } from '@playwright/test';
import { resetScenario } from './support/mock';
import { authenticate } from './support/wizard';

// No Setup row: setup is a full-screen wizard the gate raises in place of the
// shell, not a destination inside it.
const NAV_LABELS = ['Overview', 'Funds', 'Advertisement', 'Allocations', 'Settings'];

test('should not redirect when already ready', async ({ page }) => {
  await resetScenario(page, 'all-clear');

  await page.goto('/');
  await authenticate(page);

  await expect(page).toHaveURL(/\/$/);
  await expect(page.getByRole('heading', { name: 'Overview' })).toBeVisible();

  for (const label of NAV_LABELS) {
    await expect(
      page.getByRole('navigation', { name: 'Sections' }).getByRole('link', { name: label })
    ).toBeVisible();
  }

  await expect(page.getByRole('navigation', { name: 'Sections' }).getByText('Setup')).toHaveCount(
    0
  );
});

test('should show the access-denied screen, not the re-auth prompt, for a 403', async ({
  page
}) => {
  // permission_denied on an authenticated get_setup_state call is a policy
  // rejection, not an invalid token — the operator must never be bounced back
  // to the token prompt for it (SPEC-flip-admin-api.md:31-33).
  await resetScenario(page, 'access-denied');

  await page.goto('/');
  await authenticate(page);

  await expect(page.getByRole('heading', { name: "This token can't access that" })).toBeVisible();
  await expect(page.getByLabel('Admin token')).not.toBeVisible();
});
