#!/usr/bin/env node
/**
 * Redoes the CSS utilities migration using @layer utilities (correct approach).
 *
 * 1. Replaces utilities.module.css with utilities.css wrapped in @layer utilities
 * 2. Adds @import to app.css entry
 * 3. Rewrites component modules: composes: X from '...' → @apply X
 *
 * Delete this script after running.
 */

import { mkdirSync, readdirSync, readFileSync, statSync, unlinkSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const ROOT = new URL('..', import.meta.url).pathname.replace(/\/$/, '');
const SHARED_STYLES = join(ROOT, 'apps/liquidity-provider/src/shared/styles');
const OLD_UTILITIES = join(SHARED_STYLES, 'utilities.module.css');
const NEW_UTILITIES = join(SHARED_STYLES, 'utilities.css');
const APP_CSS = join(ROOT, 'apps/liquidity-provider/src/app/app.css');
const _OLD_IMPORT = `'@/shared/styles/utilities.module.css'`;

// ── Utility definitions ─────────────────────────────────────────────────────

const SECTIONS = [
  [
    '/* Layout — stacks */',
    {
      stack1: 'flex flex-col gap-1',
      stack2: 'flex flex-col gap-2',
      stack3: 'flex flex-col gap-3',
      stack4: 'flex flex-col gap-4',
      stack6: 'flex flex-col gap-6',
      stack8: 'flex flex-col gap-8'
    }
  ],
  [
    '/* Layout — rows */',
    {
      row2: 'flex gap-2',
      wrapRow2: 'flex flex-wrap gap-2',
      wrapRow3: 'flex flex-wrap gap-3',
      listSm: 'flex flex-col gap-1 text-sm'
    }
  ],
  [
    '/* Typography */',
    {
      pageHeading: 'text-xl font-medium',
      mutedSm: 'text-sm text-ink/60',
      mutedXs: 'text-xs text-ink/60',
      mutedXsFaint: 'text-xs text-ink/50',
      inkLabel: 'text-sm font-medium text-ink',
      label: 'text-sm font-medium',
      subSm: 'text-sm font-normal text-ink/60',
      sectionHeading: 'text-base font-semibold',
      noteXs: 'text-xs font-normal text-ink/50',
      errorLabel: 'text-xs font-semibold text-error',
      subText: 'mt-1 text-sm font-normal text-ink/50',
      intro: 'mt-2 text-sm text-ink/70'
    }
  ],
  [
    '/* Structural */',
    {
      centeredPage: 'flex min-h-screen items-center justify-center bg-surface p-6 text-ink',
      tableWrap: 'overflow-hidden rounded-card border border-muted',
      modalCard: 'w-full max-w-md rounded-card border border-muted p-6',
      confirmDanger: 'flex flex-col gap-2 rounded-card bg-errorSoft p-3',
      emptyStateSm: 'p-4 text-sm text-ink/60',
      removeBtn:
        'flex h-12 w-12 flex-none items-center justify-center rounded-full border-[1.5px] border-strong text-gray-500 hover:bg-gray-50'
    }
  ]
];

// ── Step 1: Write utilities.css ─────────────────────────────────────────────

function buildUtilitiesFile() {
  const lines = [
    '/* Shared CSS utilities — apps/liquidity-provider.',
    ' * Import via app.css; @apply these in component modules.',
    ' */',
    '',
    '@layer utilities {'
  ];
  for (const [comment, classes] of SECTIONS) {
    lines.push(`  ${comment}`);
    for (const [name, value] of Object.entries(classes)) {
      lines.push(`  .${name} { @apply ${value}; }`);
    }
    lines.push('');
  }
  lines.push('}');
  return `${lines.join('\n')}\n`;
}

mkdirSync(SHARED_STYLES, { recursive: true });
writeFileSync(NEW_UTILITIES, buildUtilitiesFile());
console.log(`✓ Written ${NEW_UTILITIES.replace(`${ROOT}/`, '')}`);

// Remove old .module.css if it exists
try {
  unlinkSync(OLD_UTILITIES);
  console.log(`✓ Removed utilities.module.css`);
} catch {}

// ── Step 2: Add @import to app.css ─────────────────────────────────────────

const appCss = readFileSync(APP_CSS, 'utf8');
if (!appCss.includes('utilities.css')) {
  const updated = appCss.replace(
    '@tailwind utilities;',
    "@tailwind utilities;\n\n@import '@/shared/styles/utilities.css';"
  );
  writeFileSync(APP_CSS, updated);
  console.log(`✓ Added @import to app.css`);
} else {
  console.log(`· app.css already imports utilities.css`);
}

// ── Step 3: Rewrite component modules ──────────────────────────────────────

function walk(dir) {
  const out = [];
  for (const name of readdirSync(dir)) {
    if (name === 'node_modules') continue;
    const full = join(dir, name);
    if (statSync(full).isDirectory()) out.push(...walk(full));
    else if (name.endsWith('.module.css') && full !== OLD_UTILITIES) out.push(full);
  }
  return out;
}

// Match:  composes: utilName from '@/shared/styles/utilities.module.css';
const COMPOSES_RE = /^(\s*)composes:\s+(\w+)\s+from\s+'@\/shared\/styles\/utilities\.module\.css';/;

function processFile(filePath) {
  const lines = readFileSync(filePath, 'utf8').split('\n');
  const result = [];
  const hits = [];

  for (const line of lines) {
    const m = line.match(COMPOSES_RE);
    if (m) {
      const indent = m[1];
      const utilName = m[2];
      result.push(`${indent}@apply ${utilName};`);
      hits.push(utilName);
      continue;
    }
    result.push(line);
  }

  if (hits.length === 0) return 0;
  writeFileSync(filePath, result.join('\n'));
  console.log(`  ${filePath.replace(`${ROOT}/`, '')}  ·  ${hits.join(', ')}`);
  return hits.length;
}

console.log('\nRewriting component modules:');
const files = walk(join(ROOT, 'apps'));
let total = 0;
for (const file of files) total += processFile(file);

console.log(`\n✓ ${total} composes → @apply replacement(s).`);
console.log('Run: node scripts/check-css-dupes.mjs  to verify clean.');
