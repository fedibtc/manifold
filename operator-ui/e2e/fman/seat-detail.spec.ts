import { expect, test } from '@playwright/test';
import { signIn } from './support/auth';
import { resetScenario } from './support/mock';

test('should show the invite code and no decommission action for a running seat', async ({
  page
}) => {
  await resetScenario(page, 'seats-mixed');

  await page.goto('/seats/seat-running-01');
  await signIn(page);

  await expect(page.getByRole('heading', { name: 'seat-running-01', level: 1 })).toBeVisible();
  await expect(page.getByText('Healthy')).toBeVisible();
  await expect(page.getByText('Invite code')).toBeVisible();
  await expect(page.getByRole('link', { name: 'Decommission seat' })).toHaveCount(0);
});

test('should mark a decommissioned seat', async ({ page }) => {
  await resetScenario(page, 'seats-mixed');

  await page.goto('/seats/seat-decommissioned-01');
  await signIn(page);

  await expect(
    page.getByRole('heading', { name: 'seat-decommissioned-01', level: 1 })
  ).toBeVisible();
  // Status chip (precedes the detail-card's "Decommissioned" date-row label).
  await expect(page.getByText('Decommissioned', { exact: true }).first()).toBeVisible();
  await expect(page.getByRole('link', { name: 'Decommission seat' })).toHaveCount(0);
});

test('should explain the FI-driven setup ceremony for a seat mid-DKG', async ({ page }) => {
  await resetScenario(page, 'seats-mixed');

  await page.goto('/seats/seat-dkg-01');
  await signIn(page);

  await expect(page.getByRole('heading', { name: 'seat-dkg-01', level: 1 })).toBeVisible();
  await expect(
    page.getByText('The setup ceremony is driven by the Federation Initiator')
  ).toBeVisible();
});

test('should reassure that an unavailable seat is recovering, not broken', async ({ page }) => {
  await resetScenario(page, 'seat-unavailable');

  await page.goto('/seats/seat-unavailable-01');
  await signIn(page);

  await expect(page.getByText('This seat is supervised and currently recovering')).toBeVisible();
  await expect(page.getByText('Unavailable', { exact: true })).toBeVisible();
});
