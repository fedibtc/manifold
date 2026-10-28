import { expect, test } from '@playwright/test';
import { signIn } from './support/auth';
import { resetScenario } from './support/mock';

test('should open a one-time QR offer from the Phone link page and cancel it', async ({
  page
}) => {
  await resetScenario(page, 'fedi-app-unlinked');
  await page.goto('/');
  await signIn(page);
  await page.getByRole('link', { name: 'Phone link', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Phone link', level: 1 })).toBeVisible();

  await page.getByRole('button', { name: 'Link a phone' }).click();
  await expect(page.getByRole('img', { name: 'Guardian link QR code' })).toBeVisible();
  await expect(page.getByText(/expires at/)).toBeVisible();
  await expect(
    page.getByText('Open the app on your phone and tap Scan.')
  ).toBeVisible();

  await page.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Link a phone' })).toBeVisible();
  await expect(page.getByRole('img', { name: 'Guardian link QR code' })).toHaveCount(0);
});
