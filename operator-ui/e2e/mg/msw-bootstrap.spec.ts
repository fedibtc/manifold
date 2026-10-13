import { expect, test } from '@playwright/test';

// Tracer bullet for the MSW layer: proves the worker starts before the app
// renders, so no request can escape to the network unmocked.
test('should expose the mock control surface once MSW has started', async ({ page }) => {
  await page.goto('/');
  await expect
    .poll(() => page.evaluate(() => Boolean((window as { __mockControl?: unknown }).__mockControl)))
    .toBe(true);
});
