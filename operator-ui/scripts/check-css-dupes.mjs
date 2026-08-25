#!/usr/bin/env node
/**
 * Finds duplicate @apply declarations across CSS module files.
 *
 * Groups selectors that share an identical @apply value — these are candidates
 * for promotion to shared utilities (app utilities.css or shared-ui styles).
 *
 * Dedupe workflow:
 *   1. Run this script to identify groups.
 *   2. Run `stylelint --fix "apps/**\/src/**\/*.module.css"` to alphabetise
 *      all blocks before extracting (normalises order so merges are clean).
 *   3. Promote repeated values to shared/styles/utilities.css via `composes`.
 *
 * Exit 0 = clean. Exit 1 = duplicates found (usable as a CI gate).
 */

import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';

const ROOT = new URL('..', import.meta.url).pathname.replace(/\/$/, '');
const SEARCH_DIR = join(ROOT, 'apps');
const MIN_OCCURRENCES = 2;

// ── Load known shared utilities ────────────────────────────────────────────
// Any single-token @apply that matches a class defined in a utilities.css is
// intentional usage, not a duplicate worth flagging. Scans every app plus
// packages/shared-ui — not hardcoded to one app — so new apps/packages don't
// need this script updated to be recognised.

function findUtilitiesFiles(dir) {
  const out = [];
  for (const name of readdirSync(dir)) {
    if (name === 'node_modules') continue;
    const full = join(dir, name);
    if (statSync(full).isDirectory()) out.push(...findUtilitiesFiles(full));
    else if (name === 'utilities.css') out.push(full);
  }
  return out;
}

function loadKnownUtilities() {
  const known = new Set();
  for (const utilPath of [
    ...findUtilitiesFiles(join(ROOT, 'apps')),
    ...findUtilitiesFiles(join(ROOT, 'packages'))
  ]) {
    const css = readFileSync(utilPath, 'utf8');
    for (const m of css.matchAll(/\.(\w+)\s*\{/g)) known.add(m[1]);
  }
  return known;
}

const KNOWN_UTILITIES = loadKnownUtilities();

function walk(dir) {
  const out = [];
  for (const name of readdirSync(dir)) {
    if (name === 'node_modules') continue;
    const full = join(dir, name);
    if (statSync(full).isDirectory()) out.push(...walk(full));
    else if (name.endsWith('.module.css')) out.push(full);
  }
  return out;
}

function extractRules(css) {
  const rules = [];
  // Match selector(s) + block body. Handles multi-line selectors with commas.
  const blockRe = /([^{}]+)\{([^{}]+)\}/g;
  while (true) {
    const m = blockRe.exec(css);
    if (m === null) break;
    const rawSelector = m[1].trim();
    const body = m[2];

    // Skip @keyframes, @media, etc.
    if (rawSelector.startsWith('@')) continue;
    // Must have at least one class selector
    if (!rawSelector.includes('.')) continue;

    const applyMatch = body.match(/@apply\s+([^;]+);/);
    if (!applyMatch) continue;

    const applyValue = applyMatch[1].replace(/\s+/g, ' ').trim();
    // Normalise multi-selector: collapse whitespace around commas
    const selector = rawSelector.replace(/\s*,\s*/g, ', ').replace(/\s+/g, ' ');

    rules.push({ selector, applyValue });
  }
  return rules;
}

// ── Build index ────────────────────────────────────────────────────────────

const files = walk(SEARCH_DIR);
/** @type {Map<string, Array<{file: string, selector: string}>>} */
const groups = new Map();

for (const file of files) {
  const css = readFileSync(file, 'utf8');
  const rel = relative(ROOT, file);
  for (const { selector, applyValue } of extractRules(css)) {
    if (!groups.has(applyValue)) groups.set(applyValue, []);
    groups.get(applyValue).push({ file: rel, selector });
  }
}

// ── Filter & report ────────────────────────────────────────────────────────

const dupes = [...groups.entries()]
  .filter(([value, hits]) => {
    if (hits.length < MIN_OCCURRENCES) return false;
    // Single-token @apply that matches a known shared utility = intentional, skip.
    if (KNOWN_UTILITIES.has(value.trim())) return false;
    return true;
  })
  .sort(([, a], [, b]) => b.length - a.length);

if (dupes.length === 0) {
  console.log('css-dupes: clean — no duplicate @apply declarations found.');
  process.exit(0);
}

console.error(`css-dupes: ${dupes.length} duplicate @apply group(s) found\n`);

for (const [applyValue, hits] of dupes) {
  console.error(`  (${hits.length}×)  @apply ${applyValue}`);
  for (const { file, selector } of hits) {
    console.error(`    ${selector.padEnd(40)}  ${file}`);
  }
  console.error();
}

console.error(
  'Promote shared values to apps/liquidity-provider/src/shared/styles/utilities.css\n' +
    'or packages/shared-ui/styles/ per the sharing ladder in CLAUDE.md.'
);

process.exit(1);
