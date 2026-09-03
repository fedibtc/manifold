import { expect, test } from '@playwright/test';
import { signIn } from './support/auth';

// @live specs run only under E2E_TARGET=daemon against a real, defe-provisioned
// Fleet Manager (via `just test-e2e-ui-fman`). They never run in mock mode.
// There is no scenario to reset here — the real daemon is the source of truth,
// and a freshly provisioned one owns no seats.
test('@live should set price and seat capacity and read them back', async ({ page }) => {
  await page.goto('/offer');
  await signIn(page);

  await expect(page.getByRole('heading', { name: 'Your offer', level: 1 })).toBeVisible();

  const maxSeats = page.getByLabel('Maximum active seats');
  await maxSeats.fill('9');
  const capacitySaved = page.waitForResponse(
    (response) => response.request().postData()?.includes('SetCapacity') ?? false
  );
  await page.getByRole('button', { name: 'Save seat limit' }).click();
  await capacitySaved;

  const price = page.getByLabel('Price per seat (sats)');
  await price.fill('77000');
  const priceSaved = page.waitForResponse(
    (response) => response.request().postData()?.includes('SetPrice') ?? false
  );
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await priceSaved;

  await page.reload();

  await expect(page.getByLabel('Price per seat (sats)')).toHaveValue('77000');
  await expect(page.getByLabel('Maximum active seats')).toHaveValue('9');
});
