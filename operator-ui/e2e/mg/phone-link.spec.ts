import { expect, test } from '@playwright/test';
import { signIn } from './support/auth';
import { resetScenario } from './support/mock';

test('should show a one-time QR code as soon as the Phone link page opens', async ({ page }) => {
  await resetScenario(page, 'fedi-app-unlinked');
  await page.goto('/');
  await signIn(page);
  await page.getByRole('link', { name: 'Phone link', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Phone link', level: 1 })).toBeVisible();

  await expect(page.getByRole('img', { name: 'Guardian link QR code' })).toBeVisible();
  await expect(page.getByText(/This code works once and expires at/)).toBeVisible();
  await expect(page.getByText(/tap Scan\/Paste and point the camera here/)).toBeVisible();
  await expect(page.getByRole('button', { name: 'Cancel', exact: true })).toHaveCount(0);
});
