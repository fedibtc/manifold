import { expect, type Page, test } from '@playwright/test';
import { signIn } from './support/auth';
import { resetScenario } from './support/mock';

const savePrice = async (page: Page) => {
  const saved = page.waitForResponse(
    (response) =>
      response.url().endsWith('/api/admin') && response.request().postData()?.includes('SetPrice')
  );
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  expect((await saved).ok()).toBe(true);
};

test('should show price and seat capacity and let the operator change the price', async ({
  page
}) => {
  await resetScenario(page, 'seats-mixed');

  await page.goto('/');
  await signIn(page);

  await expect(page.getByText('50,000 sats per seat')).toBeVisible();
  await page.getByRole('link', { name: 'Change price and seats' }).click();

  await expect(page.getByRole('heading', { name: 'Your offer', level: 1 })).toBeVisible();
  await expect(page.getByLabel('Maximum active seats')).toHaveValue('3');
  const price = page.getByLabel('Price per seat (sats)');
  await expect(price).toHaveValue('50000');

  await price.fill('25000');
  await savePrice(page);

  await page.goto('/');
  await expect(page.getByText('25,000 sats per seat')).toBeVisible();
});

test('should offer seats free at a price of zero rather than stopping the sale', async ({
  page
}) => {
  await resetScenario(page, 'seats-mixed');

  await page.goto('/offer');
  await signIn(page);

  await page.getByLabel('Price per seat (sats)').fill('0');
  await savePrice(page);

  await page.goto('/');
  await expect(page.getByText('Free', { exact: true })).toBeVisible();
});

test('should stop selling when the price is cleared', async ({ page }) => {
  await resetScenario(page, 'seats-mixed');

  await page.goto('/offer');
  await signIn(page);

  await page.getByLabel('Price per seat (sats)').fill('');
  await savePrice(page);

  await page.goto('/');
  await expect(page.getByText('Not selling seats')).toBeVisible();
});

test('should reject a fractional price without leaving the form', async ({ page }) => {
  await resetScenario(page, 'seats-mixed');

  await page.goto('/offer');
  await signIn(page);

  await page.getByLabel('Price per seat (sats)').fill('12.5');
  await page.getByRole('button', { name: 'Save', exact: true }).click();

  await expect(page.getByText('Sats cannot be fractional.')).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Your offer', level: 1 })).toBeVisible();
});

test('should warn when a paid offer has nowhere to receive payment', async ({ page }) => {
  await resetScenario(page, 'offer-without-payments');

  await page.goto('/');
  await signIn(page);

  await expect(page.getByText('Seats are priced but cannot be paid for')).toBeVisible();
  await expect(page.getByRole('link', { name: 'Review' })).toHaveAttribute('href', '/offer');
});
