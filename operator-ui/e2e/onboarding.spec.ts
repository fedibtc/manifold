import { expect, test } from '@playwright/test';
import { resetScenario } from './support/mock';
import { authenticate, completeWizard } from './support/wizard';

test('should raise a full-screen wizard, complete it, apply, and lift the gate', async ({
  page
}) => {
  await resetScenario(page, 'setup-fresh');

  await page.goto('/');
  await authenticate(page);

  // Gated: the wizard replaces the shell outright. Setup owns no route, so
  // the operator's location is untouched — only the nav's absence says the
  // gate is up.
  await expect(page.getByRole('heading', { name: 'Setup — Network' })).toBeVisible();
  await expect(page.getByRole('navigation', { name: 'Sections' })).toHaveCount(0);
  await expect(page.getByRole('link', { name: 'Overview' })).toHaveCount(0);

  await completeWizard(page);

  await page.getByRole('button', { name: 'Re-run validation' }).click();
  await expect(page.getByText('gateway_reachability')).toBeVisible();

  await page.getByRole('button', { name: 'Apply & go live' }).click();

  // Applied: the live screen shows. The gate stays latched to the full-screen
  // wizard (no shell, no nav) until the operator leaves it, so the "you're
  // live" screen survives the status flip to ready.
  await expect(page.getByText("You're live")).toBeVisible();
  await expect(page.getByRole('navigation', { name: 'Sections' })).toHaveCount(0);

  await page.getByRole('button', { name: 'Go to overview' }).click();

  // Leaving the live screen drops the gate: the shell mounts and the nav
  // appears.
  await expect(page.getByRole('heading', { name: 'Overview' })).toBeVisible();
  await expect(page.getByRole('link', { name: 'Overview' })).toBeVisible();
});
