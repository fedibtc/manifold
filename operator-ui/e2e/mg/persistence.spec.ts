import { expect, test } from '@playwright/test';
import { signIn } from './support/auth';
import { resetScenario } from './support/mock';

// Headline capability of the scenario store: mutations persist to
// localStorage, so a page reload must not discard them. A running seat has no
// decommission link in the UI (see seat-detail.spec.ts), so the mutation is
// driven straight through the admin API MSW serves — that is the system under
// test here, not the UI affordance.
test('should keep a seat decommissioned after reloading the page', async ({ page }) => {
  await resetScenario(page, 'seats-mixed');

  await page.goto('/seats/seat-running-01');
  await signIn(page);
  // Wait for the signed-in view before mutating — the sign-in click does not
  // await its underlying request, so an immediate fetch would race the login.
  await expect(page.getByRole('heading', { name: 'seat-running-01', level: 1 })).toBeVisible();

  await page.evaluate(async () => {
    await fetch('/api/admin', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ DecommissionSeat: { seat_id: 'seat-running-01' } })
    });
  });

  await page.reload();

  await expect(page.getByRole('heading', { name: 'seat-running-01', level: 1 })).toBeVisible();
  await expect(page.getByText('Decommissioned', { exact: true }).first()).toBeVisible();
});
