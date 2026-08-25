import { expect, test } from '@playwright/test';
import { signIn } from './support/auth';
import { resetScenario } from './support/mock';

// The takeover is advisory. These specs are mostly about what it does NOT do:
// it does not block the dashboard, it does not survive a dismissal, and it does
// not outlive the session it was dismissed in.

const TAKEOVER_HEADING = 'Update this Fleet Manager';
const DISMISS_LABEL = 'Continue to the dashboard';

test('should take the screen over when the daemon reports a newer release', async ({ page }) => {
  await resetScenario(page, 'fman-update-required');

  await page.goto('/');
  await signIn(page);

  await expect(page.getByRole('heading', { name: TAKEOVER_HEADING, level: 1 })).toBeVisible();
  await expect(page.getByText('0.1.0')).toBeVisible();
  await expect(page.getByText('0.2.0')).toBeVisible();
});

test('should hand the dashboard back when dismissed', async ({ page }) => {
  await resetScenario(page, 'fman-update-required');

  await page.goto('/');
  await signIn(page);
  await page.getByRole('button', { name: DISMISS_LABEL }).click();

  await expect(page.getByRole('heading', { name: TAKEOVER_HEADING })).toHaveCount(0);
  await expect(page.getByRole('heading', { name: 'Overview', level: 1 })).toBeVisible();
});

// The dismissal lives in React state, so navigating must not resurrect it —
// and nothing may write it to storage, or "for this session" becomes "forever".
test('should stay dismissed while the operator moves around the dashboard', async ({ page }) => {
  await resetScenario(page, 'fman-update-required');

  await page.goto('/');
  await signIn(page);
  await page.getByRole('button', { name: DISMISS_LABEL }).click();
  await page
    .getByRole('navigation', { name: 'Sections' })
    .getByRole('link', { name: 'Seats' })
    .click();

  await expect(page.getByRole('heading', { name: 'Seats', level: 1 })).toBeVisible();
  await expect(page.getByRole('heading', { name: TAKEOVER_HEADING })).toHaveCount(0);
});

test('should return after a reload, because the update is still not installed', async ({
  page
}) => {
  await resetScenario(page, 'fman-update-required');

  await page.goto('/');
  await signIn(page);
  await page.getByRole('button', { name: DISMISS_LABEL }).click();
  await expect(page.getByRole('heading', { name: TAKEOVER_HEADING })).toHaveCount(0);

  await page.reload();

  await expect(page.getByRole('heading', { name: TAKEOVER_HEADING, level: 1 })).toBeVisible();
});

test('should never appear for a fleet running the published release', async ({ page }) => {
  await resetScenario(page, 'seats-mixed');

  await page.goto('/');
  await signIn(page);

  await expect(page.getByRole('heading', { name: 'Overview', level: 1 })).toBeVisible();
  await expect(page.getByRole('heading', { name: TAKEOVER_HEADING })).toHaveCount(0);
});
