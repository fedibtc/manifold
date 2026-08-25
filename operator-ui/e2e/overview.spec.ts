import { expect, test } from '@playwright/test';
import { resetScenario } from './support/mock';
import { authenticate } from './support/wizard';

test('should aggregate a healthy snapshot into the overview hub', async ({ page }) => {
  await resetScenario(page, 'all-clear');

  await page.goto('/');
  await authenticate(page);

  await expect(page.getByRole('heading', { name: 'Overview' })).toBeVisible();

  // Status sentence for an all-clear system.
  await expect(page.getByText('All systems operational')).toBeVisible();

  // Four metric tiles.
  await expect(page.getByText('3,250,000 sats')).toBeVisible();
  await expect(page.getByText('Published')).toBeVisible();
  await expect(page.getByText('4/4 components healthy')).toBeVisible();

  // Recent-activity row from the wallet operations feed.
  await expect(page.getByText('1,000,000 sats')).toBeVisible();

  // No attention block when everything is healthy.
  await expect(page.getByText('Needs attention')).toHaveCount(0);
});

test('should raise an attention item for a critical funds scenario', async ({ page }) => {
  await resetScenario(page, 'funds-critical');

  await page.goto('/');
  await authenticate(page);

  await expect(page.getByRole('heading', { name: 'Overview' })).toBeVisible();
  await expect(page.getByText('Action required')).toBeVisible();
  await expect(page.getByText('Needs attention')).toBeVisible();
  await expect(page.getByText('Available balance critically low')).toBeVisible();
});

test('should surface degraded system health on the overview', async ({ page }) => {
  await resetScenario(page, 'health-degraded');

  await page.goto('/');
  await authenticate(page);

  await expect(page.getByRole('heading', { name: 'Overview' })).toBeVisible();

  // An unhealthy component snapshot is a critical attention item.
  await expect(page.getByText('Action required')).toBeVisible();
  await expect(page.getByText('1 item needs your attention.')).toBeVisible();
  await expect(page.getByText('System components degraded')).toBeVisible();

  // Health tile reports two of the four components as healthy.
  await expect(page.getByText('2/4 components healthy')).toBeVisible();
});
