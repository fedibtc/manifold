---
name: visual-qa
description: Figma- or screenshot-guided implementation and QA of UI. Use when implementing designs, verifying UI matches a design, or when the user provides a Figma link or screenshot.
---

# Visual QA loop

Goal: implemented UI matches the design source, verified by pixels, not by hope.

## With a Figma source (Figma MCP connected)

1. `get_design_context` (and `get_screenshot`) on the Figma node — extract layer tree, design tokens, Auto Layout rules, and the reference image.
2. Implement using REAL tokens (spacing, color, type) from the design context — never eyeball values. Standards in docs/clean-code.md apply fully.
3. Boot the dev server (project's task runner). Navigate to the route via Playwright (or browser MCP), resize viewport to the Figma frame's dimensions, screenshot.
4. Compare implementation screenshot against the Figma reference. List concrete discrepancies (spacing, color, type, alignment) with expected → actual values.
5. Fix, re-screenshot, repeat until visually faithful. Then lock it in: add/update a Playwright `toHaveScreenshot()` baseline so future drift fails deterministically in the gate.

## Screenshot-only (no Figma)

Same loop from step 3, using the provided screenshot as reference. State assumed dimensions/tokens explicitly before implementing.

## Rules

- Never claim visual parity without an actual side-by-side screenshot comparison.
- Responsive work: repeat capture at each breakpoint the design specifies.
- Commit updated `toHaveScreenshot()` baselines together with the change (tests accompany code, §8).
