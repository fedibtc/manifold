import { expect, test } from '@playwright/test';
import { resetScenario } from './support/mock';
import { authenticate } from './support/wizard';

test('should list allocations and expand an inline timeline on selection', async ({ page }) => {
  await resetScenario(page, 'allocations-mixed');

  await page.goto('/');
  await authenticate(page);

  await page
    .getByRole('navigation', { name: 'Sections' })
    .getByRole('link', { name: 'Allocations' })
    .click();
  await expect(page).toHaveURL(/\/allocations$/);
  await expect(page.getByRole('heading', { name: 'Allocations', level: 1 })).toBeVisible();

  const row = page.getByRole('button', { name: 'fed-0001' });
  await expect(row).toBeVisible();

  await expect(page.getByRole('heading', { name: 'In flight — fed-0001' })).toHaveCount(0);

  await row.click();

  await expect(page.getByRole('heading', { name: 'In flight — fed-0001' })).toBeVisible();
  await expect(page.getByText('Deposit').first()).toBeVisible();
});

test('should tag an action-required allocation and offer to cancel it', async ({ page }) => {
  await resetScenario(page, 'allocations-action-required');

  await page.goto('/allocations');
  await authenticate(page);

  await expect(page.getByRole('heading', { name: 'Allocations', level: 1 })).toBeVisible();

  // summaryStatus collapses gateway_status='action_required' to the warn chip.
  await expect(page.getByText('Action required')).toBeVisible();

  // action_required is in CANCELLABLE_STATUSES, so the timeline offers a cancel.
  await page.getByRole('button', { name: 'fed-0001' }).click();
  await expect(page.getByRole('button', { name: 'Cancel allocation' })).toBeVisible();
});

test('should tag a cancelled allocation and hide the cancel action', async ({ page }) => {
  await resetScenario(page, 'allocations-cancelled');

  await page.goto('/allocations');
  await authenticate(page);

  await expect(page.getByRole('heading', { name: 'Allocations', level: 1 })).toBeVisible();

  // summaryStatus collapses gateway_status='cancelled' to the 'Cancelled' chip.
  await expect(page.getByText('Cancelled')).toBeVisible();

  // cancelled is not in CANCELLABLE_STATUSES, so no cancel affordance appears.
  await page.getByRole('button', { name: 'fed-0001' }).click();
  await expect(page.getByRole('button', { name: 'Cancel allocation' })).toHaveCount(0);
});
