import { expect, test } from '@playwright/test';
import { signIn } from './support/auth';
import { resetScenario } from './support/mock';

test('should show the stored price and let the operator change it', async ({ page }) => {
  await resetScenario(page, 'seats-mixed');

  await page.goto('/');
  await signIn(page);

  await expect(page.getByText('50,000 sats per seat')).toBeVisible();
  await page.getByRole('link', { name: 'Change offer' }).click();

  await expect(page.getByRole('heading', { name: 'Your offer', level: 1 })).toBeVisible();
  const price = page.getByLabel('Price per seat (sats)');
  await expect(price).toHaveValue('50000');

  await price.fill('25000');
  await page.getByRole('button', { name: 'Save' }).click();

  await expect(page.getByText('25,000 sats per seat')).toBeVisible();
});

test('should offer seats free at a price of zero rather than stopping the sale', async ({
  page
}) => {
  await resetScenario(page, 'seats-mixed');

  await page.goto('/offer');
  await signIn(page);

  await page.getByLabel('Price per seat (sats)').fill('0');
  await page.getByRole('button', { name: 'Save' }).click();

  await expect(page.getByText('Free', { exact: true })).toBeVisible();
});

test('should stop selling when the price is cleared', async ({ page }) => {
  await resetScenario(page, 'seats-mixed');

  await page.goto('/offer');
  await signIn(page);

  await page.getByLabel('Price per seat (sats)').fill('');
  await page.getByRole('button', { name: 'Save' }).click();

  await expect(page.getByText('Not selling seats')).toBeVisible();
});

test('should reject a fractional price without leaving the form', async ({ page }) => {
  await resetScenario(page, 'seats-mixed');

  await page.goto('/offer');
  await signIn(page);

  await page.getByLabel('Price per seat (sats)').fill('12.5');
  await page.getByRole('button', { name: 'Save' }).click();

  await expect(page.getByText('Sats cannot be fractional.')).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Your offer', level: 1 })).toBeVisible();
});

// The gap this page was opened to close: after setup the seat ceiling was not
// readable or writable anywhere, and a store install has no command line.
test('should show the stored seat ceiling and let the operator raise it', async ({ page }) => {
  await resetScenario(page, 'seats-mixed');

  await page.goto('/offer');
  await signIn(page);

  const maxSeats = page.getByLabel('Maximum active seats');
  await expect(maxSeats).toHaveValue('3');
  await expect(page.getByText('Currently 3, with no free slots left.')).toBeVisible();

  await maxSeats.fill('8');
  await page.getByRole('button', { name: 'Save' }).click();

  await expect(page.getByRole('heading', { name: 'Overview', level: 1 })).toBeVisible();
  await page.goto('/offer');
  await expect(page.getByText('Currently 8, with 5 free.')).toBeVisible();
});

// The daemon owns this floor: the active seat count is not on the wire, so the
// screen reports the refusal rather than re-deriving the rule.
test('should refuse a ceiling below the active seats and stay on the form', async ({ page }) => {
  await resetScenario(page, 'seats-mixed');

  await page.goto('/offer');
  await signIn(page);

  await page.getByLabel('Maximum active seats').fill('2');
  await page.getByRole('button', { name: 'Save' }).click();

  await expect(page.getByText('cannot set max seats to 2; 3 seats are active')).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Your offer', level: 1 })).toBeVisible();
  await expect(page.getByText('Currently 3, with no free slots left.')).toBeVisible();
});

test('should warn when a paid offer has nowhere to receive payment', async ({ page }) => {
  await resetScenario(page, 'offer-without-payments');

  await page.goto('/');
  await signIn(page);

  await expect(page.getByText('Seats are priced but cannot be paid for')).toBeVisible();
  await expect(page.getByRole('link', { name: 'Review' })).toHaveAttribute('href', '/offer');
});
