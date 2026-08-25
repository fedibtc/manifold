import type { Page } from '@playwright/test';

const STORE_KEY = 'operator-ui:dev:mocks:fman';

// Two paths, because 20+ specs switch scenario after navigating and expect the
// change to take effect the way the old express control route did.
export const resetScenario = async (page: Page, name: string): Promise<void> => {
  if (page.url() === 'about:blank') {
    // Not navigated yet: seed storage before the app boots, so the very first
    // query already sees the right world. addInitScript re-runs on EVERY
    // document the page loads for the rest of the test (including a later
    // page.reload()) — guard on the key being absent so a reload doesn't
    // stomp state the store has since persisted.
    await page.addInitScript(
      ([key, scenario]) => {
        if (!window.localStorage.getItem(key)) {
          window.localStorage.setItem(key, JSON.stringify({ seed: scenario }));
        }
      },
      [STORE_KEY, name]
    );
    return;
  }

  await page.evaluate((scenario) => {
    const control = window.__mockControl;
    if (!control) throw new Error('mock control surface is not available');
    control.setScenario(scenario);
  }, name);
};
