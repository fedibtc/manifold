import { expect, test } from '@playwright/test';
import { signIn } from './support/auth';
import { resetScenario } from './support/mock';

const NAV_LABELS = ['Overview', 'Authorization', 'Seats', 'Wallet', 'Backup'];
const RETIRED_NAV_LABELS = ['Plans', 'Identity'];

test('should reach the fleet overview after signing in', async ({ page }) => {
  await resetScenario(page, 'seats-mixed');

  await page.goto('/');
  await signIn(page);

  await expect(page).toHaveURL(/\/$/);
  await expect(page.getByRole('heading', { name: 'Overview', level: 1 })).toBeVisible();

  for (const label of NAV_LABELS) {
    await expect(
      page.getByRole('navigation', { name: 'Sections' }).getByRole('link', { name: label })
    ).toBeVisible();
  }

  for (const label of RETIRED_NAV_LABELS) {
    await expect(
      page.getByRole('navigation', { name: 'Sections' }).getByRole('link', { name: label })
    ).toHaveCount(0);
  }
});

test('should reject an incorrect operator password', async ({ page }) => {
  await resetScenario(page, 'seats-mixed');

  await page.goto('/');
  await signIn(page, 'wrong-password');

  await expect(page.getByText('Incorrect password. Try again.')).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Sign in', level: 1 })).toBeVisible();
});
