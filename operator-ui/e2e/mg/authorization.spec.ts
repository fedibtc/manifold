import { expect, test } from '@playwright/test';
import { signIn } from './support/auth';
import { resetScenario } from './support/mock';

// The daemon reports four authorization states, not two. These three specs pin
// the ones a real browser could not previously reach: before this the dashboard
// answered "not observed" for a fleet nobody had authorized, a fleet whose relay
// had not been read, and a fleet whose relay read had failed.

test('should report a completed read that found no authorization', async ({ page }) => {
  await resetScenario(page, 'awaiting-authorization');

  await page.goto('/authorization');
  await signIn(page);

  await expect(page.getByText(/Not approved yet/i)).toBeVisible();
  // The Overview may now be definite about it, where it used to hedge.
  await page.goto('/');
  await expect(page.getByText('This host is not approved yet')).toBeVisible();
});

// Nothing stored and no read since startup: the daemon reads only when asked,
// so this does not clear on its own, and a host set up without a badge stays
// here. It is known to be unadvertised; whether the relay holds an approval is
// not, so the banner does not claim a read happened.
test('should report a host with no stored approval and no read as unadvertised', async ({
  page
}) => {
  await resetScenario(page, 'authorization-checking');

  await page.goto('/authorization');
  await signIn(page);

  await expect(page.getByText(/not been checked since the app started/i)).toBeVisible();
  await expect(page.getByText(/Last checked/i)).not.toBeVisible();

  await page.goto('/');
  await expect(page.getByText('This host is not approved yet')).toBeVisible();
});

test('should report a failed relay read as a failure, not as a missing authorization', async ({
  page
}) => {
  await resetScenario(page, 'authorization-relay-error');

  await page.goto('/authorization');
  await signIn(page);

  await expect(page.getByText(/Approval could not be checked/i)).toBeVisible();
  await expect(page.getByText(/connection refused/i)).toBeVisible();
  await expect(page.getByText(/Not approved yet/i)).not.toBeVisible();

  await page.goto('/');
  await expect(page.getByText('Approval could not be checked').first()).toBeVisible();
  await expect(page.getByText('This host is not approved yet')).not.toBeVisible();
});

// Guardian codes are a network feature: production has a badge path and nothing
// else, so a badged host there must not be told codes work.
test('should not mention guardian codes to a badged host in production', async ({ page }) => {
  await resetScenario(page, 'authorization-observed-production');

  await page.goto('/authorization');
  await signIn(page);

  await expect(page.getByText('Approved', { exact: true })).toBeVisible();
  await expect(page.getByText(/guardian code/i)).toHaveCount(0);

  await page.goto('/');
  await expect(page.getByText('Fedi-verified selection')).toBeVisible();
  await expect(page.getByText(/guardian code/i)).toHaveCount(0);
});
