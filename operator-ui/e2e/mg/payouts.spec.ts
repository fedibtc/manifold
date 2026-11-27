import { expect, type Page, test } from '@playwright/test';
import { signIn } from './support/auth';
import { resetScenario } from './support/mock';

const section = (page: Page, title: string) =>
  page.locator('section').filter({ has: page.getByRole('heading', { name: title, exact: true }) });

// Every balance row ends in a button that reads "Withdraw", so each step says
// which section it means.
const withdraw = (page: Page, title: string) =>
  section(page, title)
    .getByRole('button', { name: /^Withdraw/ })
    .first();

// The tracer for money-out: one dialog per withdrawal, Lightning or ecash,
// review then progress then receipt, and a History row once it is final.
//
// Mock tier. Nothing here is evidence about a real daemon; the MG formation
// e2e test drives the ecash verbs against a real federation.

test('should add a payout address, then withdraw over Lightning to a receipt', async ({ page }) => {
  await resetScenario(page, 'payouts-unset');

  await page.goto('/payouts');
  await signIn(page);

  await expect(page.getByRole('heading', { name: 'Payouts', level: 1 })).toBeVisible();
  await page.getByRole('button', { name: 'Add payout address' }).click();
  const address = page.getByRole('dialog', { name: 'Add payout address' });
  await address.getByLabel('Lightning address or LNURL-pay').fill('operator@example.com');
  await address.getByRole('button', { name: 'Save address' }).click();
  await expect(page.getByText('Lightning payout address')).toBeVisible();

  await withdraw(page, 'Seat sales').click();
  const dialog = page.getByRole('dialog', { name: 'Withdraw seat sales' });
  await expect(dialog.getByRole('radio', { name: 'Lightning' })).toHaveAttribute(
    'aria-checked',
    'true'
  );
  await expect(dialog.getByText('You receive about 149,250 sats')).toBeVisible();
  // No amount field and no gateway picker: a withdrawal takes the largest
  // economically fundable amount through a gateway the daemon selects.
  await expect(dialog.getByRole('textbox')).toHaveCount(0);
  await expect(dialog.getByRole('combobox')).toHaveCount(0);
  await dialog.getByRole('button', { name: 'Withdraw 149,250 sats' }).click();

  await expect(dialog.getByText('Sent 149,250 sats')).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Copy operation ID' })).toBeVisible();
  await dialog.getByRole('button', { name: 'Done' }).click();

  await expect(section(page, 'History').getByText('Sent', { exact: true })).toBeVisible();
});

// A collection reports what it COULD take: locked deposits leave only at the
// next cycle turnover, so the outcome names both figures.
test('should collect the fee pool inside the guardian-fees dialog', async ({ page }) => {
  await resetScenario(page, 'payouts-unset');

  await page.goto('/payouts');
  await signIn(page);

  await withdraw(page, 'Guardian fees').click();
  const dialog = page.getByRole('dialog', { name: 'Withdraw guardian fees' });
  await expect(dialog.getByText(/are still in the fee pool/)).toBeVisible();
  await dialog.getByRole('button', { name: /^Collect/ }).click();

  await expect(dialog.getByText(/Claimed 13,000 sats/)).toBeVisible();
  await expect(
    dialog.getByText(/3,000 sats stay locked until the next cycle turnover/)
  ).toBeVisible();
});

// The mint charges per ecash note, so a 93 sat collection gives back visibly
// less than the pool showed. The operator reads that beside the button.
test('should warn about mint fees beside a dust-sized collection', async ({ page }) => {
  await resetScenario(page, 'fees-dust');

  await page.goto('/payouts');
  await signIn(page);

  await withdraw(page, 'Guardian fees').click();
  const dialog = page.getByRole('dialog', { name: 'Withdraw guardian fees' });
  await expect(dialog.getByText(/about 0.1 sat for every note/)).toBeVisible();
  await dialog.getByRole('button', { name: 'Collect 93 sats' }).click();

  await expect(dialog.getByText(/Claimed 73 sats/)).toBeVisible();
});

// Ecash leaves to the operator's own wallet, so it needs no payout address. The
// code shows only in the dialog; closing it loses nothing.
test('should withdraw as ecash without a payout address, reopen the code, and mark it redeemed', async ({
  page
}) => {
  await resetScenario(page, 'payouts-unset');

  await page.goto('/payouts');
  await signIn(page);

  await withdraw(page, 'Seat sales').click();
  const dialog = page.getByRole('dialog', { name: 'Withdraw seat sales' });
  await expect(dialog.getByRole('radio', { name: 'Ecash' })).toHaveAttribute(
    'aria-checked',
    'true'
  );
  await dialog.getByRole('button', { name: 'Withdraw 150,000 sats as ecash' }).click();

  await expect(dialog.getByText('150,000 sats of ecash')).toBeVisible();
  await expect(dialog.getByRole('img', { name: /Ecash QR code/ })).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Copy ecash' })).toBeVisible();
  await dialog.getByRole('button', { name: 'Close' }).click();

  const inProgress = section(page, 'In progress');
  await expect(inProgress.getByText('Waiting to be redeemed')).toBeVisible();
  await expect(page.getByRole('img', { name: /Ecash QR code/ })).toHaveCount(0);
  await inProgress.getByRole('button', { name: 'Show ecash' }).click();

  await dialog.getByRole('button', { name: "I've redeemed it" }).click();
  await expect(dialog.getByText('Redeemed 150,000 sats')).toBeVisible();
  await dialog.getByRole('button', { name: 'Done' }).click();

  await expect(section(page, 'In progress')).toHaveCount(0);
  await expect(section(page, 'History').getByText('Redeemed', { exact: true })).toBeVisible();
});

test('should take an unredeemed ecash payout back into its balance', async ({ page }) => {
  await resetScenario(page, 'ecash-payout-open');

  await page.goto('/payouts');
  await signIn(page);

  await section(page, 'In progress').getByRole('button', { name: 'Show ecash' }).click();
  const dialog = page.getByRole('dialog', { name: 'Withdraw seat sales' });
  await dialog.getByRole('button', { name: 'Cancel withdrawal' }).click();
  await expect(dialog.getByText('Cancel this withdrawal?')).toBeVisible();
  await dialog.getByRole('button', { name: 'Cancel withdrawal' }).click();

  await expect(dialog.getByText('Returned 40,000 sats')).toBeVisible();
  await dialog.getByRole('button', { name: 'Done' }).click();
  await expect(section(page, 'History').getByText('Returned to balance')).toHaveCount(2);
});
