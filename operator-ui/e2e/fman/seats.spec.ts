import { expect, test } from '@playwright/test';
import { signIn } from './support/auth';
import { resetScenario } from './support/mock';

test('should explain the empty seats state', async ({ page }) => {
  await resetScenario(page, 'seats-empty');

  await page.goto('/seats');
  await signIn(page);

  await expect(page.getByRole('heading', { name: 'Seats', level: 1 })).toBeVisible();
  await expect(
    page.getByText('No seats yet. Seats are created by Federation Initiators')
  ).toBeVisible();
});

test('should render every seat phase for a mixed fleet', async ({ page }) => {
  await resetScenario(page, 'seats-mixed');

  await page.goto('/seats');
  await signIn(page);

  await expect(page.getByText('3 active · 1 decommissioned')).toBeVisible();
  await expect(page.getByRole('link', { name: 'seat-running-01' })).toBeVisible();
  await expect(page.getByRole('cell', { name: 'Running', exact: true })).toBeVisible();
  await expect(page.getByRole('cell', { name: 'DKG in progress', exact: true })).toBeVisible();
  await expect(page.getByRole('cell', { name: 'Created', exact: true })).toBeVisible();
  // A decommissioned seat renders — for phase/health; it stays listed by id.
  // Seat ids render middle-truncated in the table (truncateMiddle 8/8), so
  // match a name regex that survives truncation.
  await expect(page.getByRole('link', { name: /seat-dec.*01/ })).toBeVisible();
});

test('should mark an unavailable seat health in the table', async ({ page }) => {
  await resetScenario(page, 'seat-unavailable');

  await page.goto('/seats');
  await signIn(page);

  await expect(page.getByRole('cell', { name: 'Unavailable', exact: true })).toBeVisible();
});
