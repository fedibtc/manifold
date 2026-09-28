import { expect, test } from '@playwright/test';
import { signIn } from './support/auth';
import { resetScenario } from './support/mock';

test('should report an advertised, healthy fleet when every federation is receivable', async ({
  page
}) => {
  await resetScenario(page, 'seats-mixed');

  await page.goto('/');
  await signIn(page);

  await expect(page.getByRole('heading', { name: 'Overview', level: 1 })).toBeVisible();
  await expect(page.getByText('Advertised and healthy')).toBeVisible();
});

test('should run the self-check on click and keep clipboard failures local', async ({ page }) => {
  await resetScenario(page, 'fresh-fleet');
  await page.addInitScript(() => {
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: {
        writeText: async () => {
          throw new Error('private-host secret');
        }
      }
    });
  });
  await page.goto('/health');
  await signIn(page);

  const preview = page.getByLabel('Report preview');
  await expect(preview).toHaveCount(0);
  await page.getByRole('button', { name: 'Run self-check' }).click();
  await expect(page.getByText('Some checks were not confirmed')).toBeVisible();
  await page.getByText('Shareable report').click();
  await expect(preview).toContainText('fman-local-health-check');
  await page.getByRole('button', { name: 'Copy report' }).click();
  await expect(page.getByText('Select and copy the report manually.').first()).toBeVisible();
  await expect(preview).not.toContainText('private-host');
});

test('should distinguish all-pass and warning results on Health', async ({ page }) => {
  await resetScenario(page, 'authorization-observed');
  await page.addInitScript(() => {
    const original = window.fetch.bind(window);
    window.fetch = async (...args) => {
      const response = await original(...args);
      if (args[1]?.body !== '"RunSelfCheck"' || !response.ok) return response;
      const payload = await response.clone().json();
      const checks = payload.Ok.report.checks.map(
        (check: { check_id: string; status: string; reason_code: string }) => ({
          ...check,
          status: 'pass',
          reason_code: check.check_id === 'fman_relay' ? 'connected' : 'reached'
        })
      );
      if (window.localStorage.getItem('health-test-warning') === 'true') {
        checks[1].status = 'warning';
        checks[1].reason_code = 'http_service';
      }
      return new Response(
        JSON.stringify({ Ok: { state: 'completed', report: { schema_version: 1, checks } } }),
        { status: 200, headers: { 'content-type': 'application/json' } }
      );
    };
  });
  await page.goto('/health');
  await signIn(page);
  await expect(page.getByRole('link', { name: 'Health' })).toHaveAttribute('aria-current', 'page');
  await expect(page.getByRole('heading', { name: 'Fleet status' })).toBeVisible();
  await page.getByRole('button', { name: 'Run self-check' }).click();
  await expect(page.getByText('All checks passed')).toBeVisible();
  await expect(page.getByText('8 checks passed.')).toBeVisible();
  await expect(page.getByLabel('Report preview')).not.toBeVisible();

  await page.evaluate(() => window.localStorage.setItem('health-test-warning', 'true'));
  await page.getByRole('button', { name: 'Run self-check' }).click();
  await expect(page.getByText('Needs attention').first()).toBeVisible();
  await expect(page.getByText('Guardian discovery HTTPS transport — Warning')).toBeVisible();
  await expect(page.getByText('All checks passed')).toHaveCount(0);
});

test('should lead with the money: balance and both revenue streams', async ({ page }) => {
  await resetScenario(page, 'earnings');

  await page.goto('/');
  await signIn(page);

  await expect(page.getByText('Held in federations')).toBeVisible();
  await expect(page.getByText('162,000 sats')).toBeVisible();
  await expect(page.getByText('Seat sales, all time')).toBeVisible();
  await expect(page.getByText('Guardian fees, all time')).toBeVisible();
});

test('should bucket earnings by day, showing seat sales and guardian fees together', async ({
  page
}) => {
  await resetScenario(page, 'earnings');

  await page.goto('/');
  await signIn(page);

  await expect(page.getByRole('heading', { name: 'Earnings', level: 2 })).toBeVisible();
  await expect(page.getByText('Seat sold').first()).toBeVisible();
  await expect(page.getByText('Guardian fee').first()).toBeVisible();
});

test('should state the network-fee and completed-payment caveats on screen', async ({ page }) => {
  await resetScenario(page, 'earnings');

  await page.goto('/');
  await signIn(page);

  await expect(
    page.getByText(/Amounts shown are what buyers paid. Network fees apply./)
  ).toBeVisible();
  await expect(
    page.getByText(/A seat sale is counted once the buyer's payment has completed/)
  ).toBeVisible();
});

test('should invite the operator to earn when nothing has landed yet', async ({ page }) => {
  await resetScenario(page, 'fresh-fleet');

  await page.goto('/');
  await signIn(page);

  await expect(page.getByText(/Nothing earned yet/)).toBeVisible();
});

test('should flag a non-receivable payment federation as needing attention', async ({ page }) => {
  await resetScenario(page, 'wallet-not-receivable');

  await page.goto('/');
  await signIn(page);

  await expect(page.getByText('Needs your attention')).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Needs attention', level: 2 })).toBeVisible();
  await expect(page.getByText('Payment federation not accepting payments')).toBeVisible();
  await expect(page.getByRole('link', { name: 'Review' }).first()).toHaveAttribute(
    'href',
    '/payouts'
  );
});
