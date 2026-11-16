import { expect, test } from '@playwright/test';
import { signIn } from './support/auth';
import { resetScenario } from './support/mock';

test('should show a one-time QR code as soon as the Mobile notifications page opens', async ({
  page
}) => {
  await resetScenario(page, 'fedi-app-unlinked');
  await page.goto('/');
  await signIn(page);
  await page.getByRole('link', { name: /^Mobile notifications/ }).click();
  await expect(
    page.getByRole('heading', { name: 'Stay connected to your guardian', level: 1 })
  ).toBeVisible();

  await expect(page.getByRole('img', { name: 'Guardian link QR code' })).toBeVisible();
  await expect(page.getByText(/one alert when a problem starts/)).toBeVisible();
  await expect(page.getByText(/tap Scan\/Paste and point the camera here/)).toBeVisible();
  await expect(page.getByRole('button', { name: 'Cancel', exact: true })).toHaveCount(0);
});

test('should bring up the code for a stopped phone from one button', async ({ page }) => {
  await resetScenario(page, 'fedi-app-terminal');
  await page.goto('/');
  await signIn(page);
  await page.getByRole('link', { name: 'Mobile notifications, needs attention' }).click();
  await expect(page.getByText('Your phone stopped getting alerts')).toBeVisible();
  await expect(page.getByRole('img', { name: 'Guardian link QR code' })).toHaveCount(0);

  await page.getByRole('button', { name: 'Reconnect phone' }).click();
  await expect(page.getByRole('img', { name: 'Guardian link QR code' })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Reconnect your phone' })).toBeVisible();
});

test('should say the phone connected when a scan lands while the page is open', async ({
  page
}) => {
  await resetScenario(page, 'fedi-app-unlinked');
  await page.goto('/');
  await signIn(page);
  await page.getByRole('link', { name: /^Mobile notifications/ }).click();
  await expect(page.getByRole('img', { name: 'Guardian link QR code' })).toBeVisible();
  await expect(page.getByText('Phone connected')).toHaveCount(0);

  await resetScenario(page, 'fedi-app-active');
  await expect(page.getByText('Phone connected')).toBeVisible();
  await expect(page.getByRole('link', { name: 'Mobile notifications', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Dismiss' }).click();
  await expect(page.getByText('Phone connected')).toHaveCount(0);
});
