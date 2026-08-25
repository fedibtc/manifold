import { expect, test } from '@playwright/test';
import { resetScenario } from './support/mock';
import { authenticate } from './support/wizard';

test('should mark an invalid attestation with an Invalid chip on settings', async ({ page }) => {
  await resetScenario(page, 'all-clear');

  await page.goto('/settings');
  await authenticate(page);

  // The seeded attestation list carries a valid: false issuer_authority entry,
  // which the panel renders with an 'Invalid' chip alongside the 'Valid' ones.
  await expect(page.getByText('Invalid', { exact: true })).toBeVisible();
  await expect(page.getByText('Valid', { exact: true }).first()).toBeVisible();
});
