import { expect, test } from '@playwright/test';
import { resetScenario } from './support/mock';
import { authenticate } from './support/wizard';

test('should render balances and wallet operations for a healthy snapshot', async ({ page }) => {
  await resetScenario(page, 'all-clear');

  await page.goto('/');
  await authenticate(page);

  await page
    .getByRole('navigation', { name: 'Sections' })
    .getByRole('link', { name: 'Funds' })
    .click();

  await expect(page.getByRole('heading', { name: 'Funds' })).toBeVisible();
  await expect(page.getByText('3,250,000 sats').first()).toBeVisible();
  await expect(page.getByText('Mock Signet Gateway')).toBeVisible();
  await expect(page.getByText('wop-0003')).toBeVisible();
  await expect(page.getByText('Critical balance')).toHaveCount(0);
});

test('should show the critical replenishment banner for the funds-critical scenario', async ({
  page
}) => {
  await resetScenario(page, 'funds-critical');

  await page.goto('/');
  await authenticate(page);

  await page
    .getByRole('navigation', { name: 'Sections' })
    .getByRole('link', { name: 'Funds' })
    .click();

  await expect(page.getByRole('heading', { name: 'Funds' })).toBeVisible();
  await expect(page.getByText('Critical balance')).toBeVisible();
});

test('should show the warning replenishment banner and chip for the funds-warning scenario', async ({
  page
}) => {
  await resetScenario(page, 'funds-warning');

  await page.goto('/funds');
  await authenticate(page);

  await expect(page.getByRole('heading', { name: 'Funds' })).toBeVisible();

  // Warning banner copy from deriveFunds.ts (REPLENISHMENT_BANNERS.warning).
  await expect(page.getByText('Replenishment recommended')).toBeVisible();
  await expect(
    page.getByText('Available balance is below the warning threshold. Top up soon.')
  ).toBeVisible();

  // Balance chip label from REPLENISHMENT_CHIPS.warning.
  await expect(page.getByText('Below warning threshold')).toBeVisible();
});

test('should label broadcast and cancelled wallet operations in the operations table', async ({
  page
}) => {
  await resetScenario(page, 'wallet-ops-broadcast-cancelled');

  await page.goto('/funds');
  await authenticate(page);

  await expect(page.getByRole('heading', { name: 'Funds' })).toBeVisible();

  // WalletOperationsTable renders status via humanizeToken (snake → spaced).
  await expect(page.getByText('broadcast')).toBeVisible();
  await expect(page.getByText('cancelled')).toBeVisible();
});

test('should label in-doubt and manual-review wallet operations in the operations table', async ({
  page
}) => {
  await resetScenario(page, 'wallet-ops-review');

  await page.goto('/funds');
  await authenticate(page);

  await expect(page.getByRole('heading', { name: 'Funds' })).toBeVisible();

  // The always-visible operations table humanizes the raw status token; the
  // "Needs review" copy lives only in TopupPanel (not shown on /funds).
  await expect(page.getByText('in doubt')).toBeVisible();
  await expect(page.getByText('manual review required')).toBeVisible();
});
