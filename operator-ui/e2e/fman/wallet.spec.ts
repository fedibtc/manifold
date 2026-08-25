import { expect, test } from '@playwright/test';
import { signIn } from './support/auth';
import { resetScenario } from './support/mock';

test('should explain the empty wallet state', async ({ page }) => {
  await resetScenario(page, 'fresh-fleet');

  await page.goto('/wallet');
  await signIn(page);

  await expect(page.getByRole('heading', { name: 'Wallet', level: 1 })).toBeVisible();
  await expect(page.getByText('No payment federations accepted yet.')).toBeVisible();
});

test('should mark a receivable payment federation with its balance', async ({ page }) => {
  await resetScenario(page, 'seats-mixed');

  await page.goto('/wallet');
  await signIn(page);

  await expect(page.getByText('Total balance: 250,000 sats')).toBeVisible();
  await expect(page.getByText('Receivable')).toBeVisible();
});

test('should flag a non-receiving payment federation', async ({ page }) => {
  await resetScenario(page, 'wallet-not-receivable');

  await page.goto('/wallet');
  await signIn(page);

  await expect(page.getByText('Not receiving')).toBeVisible();
});

// Membership is not an operator choice, so no row adds or removes one. Money-out
// is not here either: the daemon's sweep verbs need a payout destination that
// this screen knows nothing about, so the whole path lives on Payouts and the
// Wallet points at it rather than carrying half of it.
test('should offer no add, remove, or money-out action on a federation row', async ({ page }) => {
  await resetScenario(page, 'seats-mixed');

  await page.goto('/wallet');
  await signIn(page);

  await expect(page.getByRole('link', { name: 'Add federation' })).toHaveCount(0);
  await expect(page.getByRole('link', { name: 'Remove' })).toHaveCount(0);
  await expect(page.getByRole('link', { name: 'Withdraw' })).toHaveCount(0);
  await expect(
    page.getByText(/Membership follows the accepted common setup-payment set/)
  ).toBeVisible();
});
