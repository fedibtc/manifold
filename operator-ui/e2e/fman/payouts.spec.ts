import { expect, test } from '@playwright/test';
import { signIn } from './support/auth';
import { resetScenario } from './support/mock';

// The tracer for money-out. It walks the ordering the daemon enforces — no sweep
// answers until a payout destination is stored — and then both revenue paths,
// which are shaped differently: a payment federation sweeps in one step, a
// seat's guardian fees collect out of the pool first and are sent second.
//
// Mock tier. Nothing here is evidence about a real daemon: the rung-M3 @live
// spec that would assert the balance changed AT the daemon is still open, and is
// blocked on the FMan live e2e tier (W0.2).

test('should refuse a sweep until a payout destination is stored, then sweep', async ({ page }) => {
  await resetScenario(page, 'payouts-unset');

  await page.goto('/payouts');
  await signIn(page);

  await expect(page.getByRole('heading', { name: 'Payouts', level: 1 })).toBeVisible();
  await expect(page.getByText('No payout destination')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Sweep' })).toBeDisabled();
  await expect(page.getByText('Set a payout destination first.').first()).toBeVisible();

  await page.getByLabel('Lightning address or LNURL-pay').fill('operator@example.com');
  await page.getByRole('button', { name: 'Save destination' }).click();

  await expect(page.getByText('No payout destination')).toBeHidden();
  await expect(page.getByRole('button', { name: 'Sweep' })).toBeEnabled();

  await page.getByRole('button', { name: 'Sweep' }).click();

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

  await page.getByRole('button', { name: '1. Collect out of the pool' }).click();

  await expect(page.getByText(/Claimed 13,000 sats/)).toBeVisible();
  await expect(
    page.getByText(/3,000 sats stay locked until the next cycle turnover/)
  ).toBeVisible();
});

test('should send collected guardian fees only after a destination exists', async ({ page }) => {
  await resetScenario(page, 'payouts-unset');

  await page.goto('/payouts');
  await signIn(page);

  await expect(page.getByRole('button', { name: '2. Send to destination' })).toBeDisabled();

  await page.getByRole('button', { name: '1. Collect out of the pool' }).click();
  await page.getByLabel('Lightning address or LNURL-pay').fill('operator@example.com');
  await page.getByRole('button', { name: 'Save destination' }).click();

  await page.getByRole('button', { name: '2. Send to destination' }).click();

  await expect(page.getByText('Sent 13,000 sats.')).toBeVisible();
});

// No amount field and no gateway picker, because the admin API exposes neither:
// a sweep takes the largest economically fundable amount through a gateway the
// daemon selects. A control for either would be a control the daemon cannot honour.
test('should offer no amount field and no gateway picker', async ({ page }) => {
  await resetScenario(page, 'earnings');

  await page.goto('/payouts');
  await signIn(page);

  await expect(page.getByRole('button', { name: 'Sweep' }).first()).toBeEnabled();
  await expect(page.getByLabel(/amount/i)).toHaveCount(0);
  await expect(page.getByRole('combobox')).toHaveCount(0);
  await expect(page.getByText(/There is no amount to enter and no gateway to pick/)).toBeVisible();
});
