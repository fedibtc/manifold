import { expect, test } from '@playwright/test';
import { resetScenario } from './support/mock';
import { authenticate } from './support/wizard';

test('should show the advertisement listing and republish against the mock', async ({ page }) => {
  await resetScenario(page, 'ad-stale');

  await page.goto('/advertisement');
  await authenticate(page);

  // Listing renders with the seeded stale publication status and a relay row.
  // Relay URLs render middle-truncated (truncateMiddle) with a copy button,
  // so match on a prefix regex that survives truncation.
  await expect(page.getByRole('heading', { name: 'Advertisement' })).toBeVisible();
  await expect(page.getByText('Stale')).toBeVisible();
  await expect(page.getByText(/wss:\/\/relay\.si.*example/)).toBeVisible();

  // Republish flips the mock to published and the header chip reflects it.
  // Scope to the page header: relay rows also render a 'Published' label once
  // republished, so an unscoped locator trips strict mode.
  const header = page.locator('header');
  await page.getByRole('button', { name: 'Republish now' }).click();
  await expect(header.getByText('Published')).toBeVisible();
});

test('should surface a failed publication status on the advertisement screen', async ({ page }) => {
  await resetScenario(page, 'ad-failed');

  await page.goto('/advertisement');
  await authenticate(page);

  // Header chip reflects the failed publication status.
  // No relay row reads 'Failed' here (seeded relays are published), so the chip
  // is the only 'Failed' on the page and needs no header scoping.
  await expect(page.getByRole('heading', { name: 'Advertisement' })).toBeVisible();
  await expect(page.getByText('Failed')).toBeVisible();

  // The listing card still renders — a failed publish keeps the signed view.
  await expect(page.getByText('Gateway (Lightning) · Stability pool')).toBeVisible();
});

test('should require confirmation before withdrawing the advertisement', async ({ page }) => {
  await resetScenario(page, 'all-clear');

  await page.goto('/advertisement');
  await authenticate(page);

  // Clicking Withdraw does not withdraw immediately — it reveals a confirm
  // panel, and the listing stays published until the operator confirms.
  const header = page.locator('header');
  await expect(header.getByText('Published')).toBeVisible();
  await page.getByRole('button', { name: 'Withdraw advertisement' }).click();
  await expect(page.getByRole('button', { name: 'Confirm withdrawal' })).toBeVisible();
  await expect(header.getByText('Published')).toBeVisible();

  // Cancel backs out without withdrawing.
  await page.getByRole('button', { name: 'Cancel' }).click();
  await expect(page.getByRole('button', { name: 'Withdraw advertisement' })).toBeVisible();
  await expect(header.getByText('Published')).toBeVisible();

  // Confirming withdraws against the mock and the header chip reflects it.
  await page.getByRole('button', { name: 'Withdraw advertisement' }).click();
  await page.getByRole('button', { name: 'Confirm withdrawal' }).click();
  await expect(header.getByText('Withdrawn')).toBeVisible();
});

test('should render connected and failed relay rows in the relays table', async ({ page }) => {
  await resetScenario(page, 'ad-relays-mixed');

  await page.goto('/advertisement');
  await authenticate(page);

  // A connected relay renders its plain status label. Scope to exact text: the
  // relay URL 'wss://relay.connected.example' also matches 'connected'. URLs
  // render middle-truncated, so match a prefix regex that survives truncation.
  await expect(page.getByText(/wss:\/\/relay\.co.*example/)).toBeVisible();
  await expect(page.getByText('Connected', { exact: true })).toBeVisible();

  // A failed relay appends its last error after the status label.
  await expect(page.getByText(/wss:\/\/relay\.fa.*example/)).toBeVisible();
  await expect(page.getByText('Failed · relay handshake rejected')).toBeVisible();
});
