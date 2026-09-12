import { expect, type Page, test } from '@playwright/test';
import { signIn } from './support/auth';
import { resetScenario } from './support/mock';

// Both revenue sections end in a button called "Withdraw", so every assertion
// about one of them says which section it means.
const section = (page: Page, title: string) =>
  page.locator('section').filter({ has: page.getByRole('heading', { name: title, exact: true }) });

const withdraw = (page: Page, title: string) =>
  section(page, title).getByRole('button', { name: 'Withdraw' });

// The tracer for money-out. It walks the ordering the daemon enforces — no sweep
// answers until a payout destination is stored — and then both revenue paths,
// which are shaped differently: a payment federation sweeps in one step, a
// seat's guardian fees collect out of the pool first and are sent second.
//
// Mock tier. Nothing here is evidence about a real daemon: the rung-M3 @live
// spec that would assert the balance changed AT the daemon is still open, and is
// blocked on the FMan live e2e tier (W0.2).

test('should refuse a withdrawal until a payout address is stored, then withdraw', async ({
  page
}) => {
  await resetScenario(page, 'payouts-unset');

  await page.goto('/payouts');
  await signIn(page);

  await expect(page.getByRole('heading', { name: 'Payouts', level: 1 })).toBeVisible();
  await expect(page.getByText('Add a payout address to withdraw')).toBeVisible();
  await expect(withdraw(page, 'Seat sales')).toBeDisabled();
  await expect(page.getByText('Add a payout address first.').first()).toBeVisible();

  await page.getByLabel('Lightning address or LNURL-pay').fill('operator@example.com');
  await page.getByRole('button', { name: 'Save destination' }).click();

  await expect(page.getByText('Add a payout address to withdraw')).toBeHidden();
  await expect(withdraw(page, 'Seat sales')).toBeEnabled();

  await withdraw(page, 'Seat sales').click();

  await expect(page.getByText('Sent 150,000 sats.')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Copy operation ID' })).toBeVisible();
});

// A collection reports what it COULD take. The locked deposits leave only at the
// next cycle turnover, so the confirmation names both figures — asserting the
// second one is the point of this test.
test('should report what a collection claimed and what is still locked', async ({ page }) => {
  await resetScenario(page, 'payouts-unset');

  await page.goto('/payouts');
  await signIn(page);

  await page.getByRole('button', { name: 'Collect fees' }).click();

  await expect(page.getByText(/Claimed 13,000 sats/)).toBeVisible();
  await expect(
    page.getByText(/3,000 sats stay locked until the next cycle turnover/)
  ).toBeVisible();
});

test('should send collected guardian fees only after a destination exists', async ({ page }) => {
  await resetScenario(page, 'payouts-unset');

  await page.goto('/payouts');
  await signIn(page);

  await expect(withdraw(page, 'Guardian fees')).toBeDisabled();

  await page.getByRole('button', { name: 'Collect fees' }).click();
  await page.getByLabel('Lightning address or LNURL-pay').fill('operator@example.com');
  await page.getByRole('button', { name: 'Save destination' }).click();

  await withdraw(page, 'Guardian fees').click();

  await expect(page.getByText('Sent 13,000 sats.')).toBeVisible();
});

// No amount field and no gateway picker, because the admin API exposes neither:
// a sweep takes the largest economically fundable amount through a gateway the
// daemon selects. A control for either would be a control the daemon cannot honour.
test('should offer no amount field and no gateway picker', async ({ page }) => {
  await resetScenario(page, 'earnings');

  await page.goto('/payouts');
  await signIn(page);

  await expect(withdraw(page, 'Seat sales').first()).toBeEnabled();
  await expect(page.getByLabel(/amount/i)).toHaveCount(0);
  await expect(page.getByRole('combobox')).toHaveCount(0);
  await expect(page.getByText(/no amount to enter, nothing to configure/)).toBeVisible();
});
