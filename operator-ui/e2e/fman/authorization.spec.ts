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

  await expect(page.getByText(/No authorization for this fleet/i)).toBeVisible();
  // The Overview may now be definite about it, where it used to hedge.
  await page.goto('/');
  await expect(page.getByText('No holder has authorized this fleet')).toBeVisible();
});

test('should say nothing is known while the first relay read is outstanding', async ({ page }) => {
  await resetScenario(page, 'authorization-checking');

  await page.goto('/authorization');
  await signIn(page);

  await expect(page.getByText(/Reading the relay for the first time/i)).toBeVisible();
  await expect(page.getByText(/No authorization for this fleet/i)).not.toBeVisible();

  // An item the operator cannot act on is noise, so the Overview raises none.
  await page.goto('/');
  await expect(page.getByText('No holder has authorized this fleet')).not.toBeVisible();
});

test('should report a failed relay read as a failure, not as a missing authorization', async ({
  page
}) => {
  await resetScenario(page, 'authorization-relay-error');

  await page.goto('/authorization');
  await signIn(page);

  await expect(page.getByText(/relay could not be read/i)).toBeVisible();
  await expect(page.getByText(/connection refused/i)).toBeVisible();
  await expect(page.getByText(/No authorization for this fleet/i)).not.toBeVisible();

  await page.goto('/');
  await expect(page.getByText('The relay could not be read')).toBeVisible();
  await expect(page.getByText('No holder has authorized this fleet')).not.toBeVisible();
});
