import type { Page } from '@playwright/test';

const STORE_KEY = 'operator-ui:dev:mocks:flip';

// Two paths, because specs switch scenario after navigating and expect the
// change to take effect the way the old express control route did.
export const resetScenario = async (page: Page, name: string): Promise<void> => {
  if (page.url() === 'about:blank') {
    // Not navigated yet: seed storage before the app boots. Only seed when the
    // key is absent — addInitScript re-runs on EVERY document in this page, and
    // re-seeding on a reload would rebuild the world and drop any mutation.
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
