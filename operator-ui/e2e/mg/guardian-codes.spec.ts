import { expect, test } from '@playwright/test';
import { signIn } from './support/auth';
import { resetScenario } from './support/mock';

const TRUST =
  'I trust the Federation Ambassador I share this code with to choose the other guardians.';

test('should create a guardian code, share it, and find it unused in the list', async ({
  page
}) => {
  await resetScenario(page, 'guardian-codes-empty');

  await page.goto('/guardian-codes');
  await signIn(page);

  await expect(page.getByText('No guardian codes yet.')).toBeVisible();
  await page.getByRole('button', { name: 'Create a guardian code' }).click();
  await page.getByLabel('Label (optional)').fill('Porto book club');
  const create = page.getByRole('button', { name: 'Create guardian code', exact: true });
  await expect(create).toBeDisabled();
  await page.getByLabel(TRUST).check();
  await create.click();

  await expect(page.getByText('Guardian code ready')).toBeVisible();
  await expect(page.getByText(/^manifold:seat-grant\?v=1&/)).toBeVisible();
  await page.getByRole('button', { name: 'Done' }).click();

  const row = page.getByRole('row', { name: /Porto book club/ });
  await expect(row.getByText('Unused')).toBeVisible();
});

test('should revoke an unused code only after confirmation', async ({ page }) => {
  await resetScenario(page, 'guardian-codes-list');

  await page.goto('/guardian-codes');
  await signIn(page);

  const row = page.getByRole('row', { name: /Lisbon meetup/ });
  await row.getByRole('button', { name: 'Revoke' }).click();
  await page.getByRole('button', { name: 'Keep code' }).click();
  await expect(row.getByText('Unused')).toBeVisible();

  await row.getByRole('button', { name: 'Revoke' }).click();
  await page.getByRole('button', { name: 'Revoke code' }).click();

  await expect(page.getByRole('dialog')).toBeHidden();
  await expect(row.getByText('Revoked')).toBeVisible();
  await expect(row.getByRole('button', { name: 'Revoke' })).toHaveCount(0);
});

test('should count unused codes and link to them from the Overview', async ({ page }) => {
  await resetScenario(page, 'guardian-codes-list');

  await page.goto('/');
  await signIn(page);

  await expect(page.getByText('Free setup · 2 unused codes')).toBeVisible();
  // The code's seat is free and never paid: it is no sale.
  await expect(page.getByText('Seat sold')).toHaveCount(1);
  await page.getByRole('link', { name: 'Manage guardian codes' }).click();
  await expect(page.getByRole('heading', { name: 'Guardian codes', level: 1 })).toBeVisible();
});
