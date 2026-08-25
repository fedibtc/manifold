import { expect, test } from '@playwright/test';
import { signIn } from './support/auth';
import { resetScenario } from './support/mock';

test('should report an advertised, healthy fleet when every federation is receivable', async ({
  page
}) => {
  await resetScenario(page, 'seats-mixed');

  await page.goto('/');
  await signIn(page);

  await expect(page.getByRole('heading', { name: 'Overview', level: 1 })).toBeVisible();
  await expect(page.getByText('Advertised and healthy')).toBeVisible();
});

test('should lead with the money: balance and both revenue streams', async ({ page }) => {
  await resetScenario(page, 'earnings');

  await page.goto('/');
  await signIn(page);

  await expect(page.getByText('Wallet balance')).toBeVisible();
  await expect(page.getByText('162,000 sats')).toBeVisible();
  await expect(page.getByText('Seat sales', { exact: true })).toBeVisible();
  await expect(page.getByText('Guardian fees', { exact: true })).toBeVisible();
});

test('should bucket earnings by day, showing seat sales and guardian fees together', async ({
  page
}) => {
  await resetScenario(page, 'earnings');

  await page.goto('/');
  await signIn(page);

  await expect(page.getByRole('heading', { name: 'Earnings', level: 2 })).toBeVisible();
  await expect(page.getByText('Seat sold').first()).toBeVisible();
  await expect(page.getByText('Guardian fee').first()).toBeVisible();
});

test('should state the gross and accepted-claim caveats on screen', async ({ page }) => {
  await resetScenario(page, 'earnings');

  await page.goto('/');
  await signIn(page);

  await expect(page.getByText(/before the mint and Lightning fees/)).toBeVisible();
  await expect(page.getByText(/accepted payment claims/)).toBeVisible();
});

test('should invite the operator to earn when nothing has landed yet', async ({ page }) => {
  await resetScenario(page, 'fresh-fleet');

  await page.goto('/');
  await signIn(page);

  await expect(page.getByText(/Nothing earned yet/)).toBeVisible();
});

test('should flag a non-receivable payment federation as needing attention', async ({ page }) => {
  await resetScenario(page, 'wallet-not-receivable');

  await page.goto('/');
  await signIn(page);

  await expect(page.getByText('Needs your attention')).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Needs attention', level: 2 })).toBeVisible();
  await expect(page.getByText('Payment federation not receiving')).toBeVisible();
  await expect(page.getByRole('link', { name: 'Review' }).first()).toHaveAttribute(
    'href',
    '/wallet'
  );
});
