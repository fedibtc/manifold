---
name: e2e
description: Playwright end-to-end test conventions. Use when writing, reviewing, or planning e2e tests, or when starting a new user-facing flow (tracer bullet).
---

# E2E conventions (docs/clean-code.md §7 applies in full)

- E2E covers **critical user journeys only** — keep the pyramid a pyramid. If it can be a unit/integration test, it is not an e2e test.
- **Tracer bullet**: a new flow's FIRST test is one e2e proving a single path end-to-end (e.g. "user can checkout with valid cart"). Build outward from it with unit tests via the red-green loop.
- Semantic locators only: `getByRole`, `getByLabel`, `getByText`. `getByTestId` requires justification.
- No `waitForTimeout` — web-first assertions auto-wait.
- No conditionals, straight-line Arrange → Act → Assert, `should…` names, SUT-pattern helpers with parameter objects.
- Visual checks: `toHaveScreenshot()` against committed baselines (see visual-qa skill).
- Specs live in `e2e/`, named for the journey: `checkout-with-valid-cart.e2e.ts`.
