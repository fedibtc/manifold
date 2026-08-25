#!/usr/bin/env node
// operator-ui folder-structure checker (feature-based layout).
//
// Enforces the layout rules ESLint boundaries and biome do NOT catch:
//   1. A page folder holds ONLY the page: apps/*/src/pages/<feature>/ may
//      contain <X>Page.tsx, <X>Page.module.css, and a __tests__/ dir —
//      nothing else. Subcomponents/utils belong in features/<feature>/.
//   2. Every feature component (apps/*/src/features/*/components/**/*.tsx) lives
//      in its own kebab-case folder AND has a sibling __tests__/<Name>.test.tsx.
//      No loose components, no untested components.
//   3. At most ONE React unit (component OR hook) is exported per file. A .tsx
//      exports one component; a hook file exports one use* hook. Type/const/plain
//      helper exports do not count, so utility files (which export neither a
//      component nor a hook) are exempt by construction — no allowlist needed.
//
// Modes (mirrors check-styles.mjs):
//   node scripts/check-structure.mjs         gate mode over CHANGED files only:
//                                             human output, exit 1 on
//                                             violations, 0 when clean.
//   node scripts/check-structure.mjs --all    same, over every tracked file.
//                                             CI runs this: the changed-files
//                                             scan is blind to violations that
//                                             are already committed, so a rule
//                                             break that lands once is never
//                                             reported again.
//   node scripts/check-structure.mjs --hook   Claude Code Stop hook: emit
//                                             {"decision":"block",...} JSON on
//                                             violations, always exit 0.
//   node scripts/check-structure.mjs --write-hook   Claude Code PostToolUse hook
//                                             (Write|Edit): inspects the single
//                                             just-written file for rule 3 only —
//                                             create-time enforcement — and emits
//                                             {"decision":"block",...} JSON.
//
// Path handling: operator-ui is a subdir of the decentralized-federations
// monorepo. git reports paths from the monorepo root; we resolve against the git
// toplevel then keep only files under this script's operator-ui root — correct
// whether invoked from the monorepo root or operator-ui.

import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { dirname, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const HOOK_MODE = process.argv.includes('--hook');
const WRITE_HOOK_MODE = process.argv.includes('--write-hook');
const ALL_MODE = process.argv.includes('--all');
const UI_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');

const run = (cmd, args, cwd) => spawnSync(cmd, args, { cwd, encoding: 'utf8', timeout: 30_000 });

const lines = (value) =>
  value
    .split(/\r?\n/)
    .map((l) => l.trim())
    .filter(Boolean);

const gitToplevel = () => {
  const result = run('git', ['rev-parse', '--show-toplevel'], UI_ROOT);
  return result.status === 0 ? result.stdout.trim() : UI_ROOT;
};

// Changed files across working tree, index, and untracked — absolute paths.
const changedFiles = (gitRoot) => {
  const commands = [
    ['git', ['diff', '--name-only', '--diff-filter=ACMR']],
    ['git', ['diff', '--cached', '--name-only', '--diff-filter=ACMR']],
    ['git', ['ls-files', '--others', '--exclude-standard']]
  ];
  const files = new Set();
  for (const [cmd, args] of commands) {
    const result = run(cmd, args, gitRoot);
    if (result.status === 0) {
      for (const file of lines(result.stdout)) files.add(resolve(gitRoot, file));
    }
  }
  return [...files];
};

// Every tracked file plus untracked ones — absolute paths. The whole-repo view
// CI needs, so an already-committed violation still fails the gate.
const allFiles = (gitRoot) => {
  const commands = [
    ['git', ['ls-files']],
    ['git', ['ls-files', '--others', '--exclude-standard']]
  ];
  const files = new Set();
  for (const [cmd, args] of commands) {
    const result = run(cmd, args, gitRoot);
    if (result.status === 0) {
      for (const file of lines(result.stdout)) files.add(resolve(gitRoot, file));
    }
  }
  return [...files];
};

const isTestOrFixture = (rel) =>
  /\.(test|spec|stories)\.(t|j)sx?$/i.test(rel) ||
  /(^|\/)(__tests__|__fixtures__|__mocks__|generated)(\/|$)/i.test(rel);

const isKebab = (name) => /^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(name);

const PAGE_RE = /^apps\/[^/]+\/src\/pages\/([^/]+)\/(.+)$/;
const COMPONENT_RE = /^apps\/[^/]+\/src\/(features)\/([^/]+)\/components\/(.+)\.tsx$/;

// Rule 1: a page feature folder holds only <X>Page.{tsx,module.css} + __tests__/.
const inspectPage = (rel) => {
  const match = rel.match(PAGE_RE);
  if (!match) return [];
  const [, feature, rest] = match;
  const segment = rest.split('/')[0];
  const isNested = rest.includes('/');

  if (segment === '__tests__') return [];
  if (isNested) {
    return [
      `${rel}: unexpected "${segment}/" inside page folder pages/${feature}/ — a page folder holds only the Page (tsx + module.css) and __tests__; move this into features/${feature}/`
    ];
  }
  if (segment.endsWith('Page.tsx') || segment.endsWith('Page.module.css')) return [];
  return [
    `${rel}: "${segment}" does not belong in a page folder — pages/${feature}/ holds only <X>Page.tsx, <X>Page.module.css, and __tests__/; move components/utils into features/${feature}/`
  ];
};

// Rule 2: a feature component sits in its own kebab folder + has a sibling test.
const inspectComponent = (absolute, rel) => {
  const match = rel.match(COMPONENT_RE);
  if (!match) return [];
  const [, , feature, tail] = match; // tail = "<...>/<Name>" (no .tsx)
  if (isTestOrFixture(rel)) return [];

  const violations = [];
  const parts = tail.split('/'); // folders... + component base
  const base = parts.at(-1);
  const folders = parts.slice(0, -1);

  if (folders.length === 0) {
    violations.push(
      `${rel}: component "${base}.tsx" sits directly in features/${feature}/components/ — put it in its own kebab-case folder (components/<kebab>/${base}.tsx)`
    );
  } else {
    const ownFolder = folders.at(-1);
    if (!isKebab(ownFolder)) {
      violations.push(
        `${rel}: component folder "${ownFolder}" is not kebab-case (e.g. allocation-status-chip)`
      );
    }
  }

  const testPath = resolve(dirname(absolute), '__tests__', `${base}.test.tsx`);
  if (!existsSync(testPath)) {
    violations.push(`${rel}: no unit test — add __tests__/${base}.test.tsx next to the component`);
  }
  return violations;
};

// Rule 3: at most one React unit (component or hook) exported per file.
// A "unit" is a component (PascalCase export in a .tsx) or a hook (use* export).
// Type/interface/plain-const/helper-function exports are NOT units, so utility
// files fall out at zero — the exception is structural, not an allowlist.
const EXPORT_SCAN_RE = /^(apps|packages)\/[^/]+\/src\//;
// Component: `export const Pascal = (`  (arrow, optional type/generic) or
//            `export function Pascal(`. The `(` guard excludes non-component
// PascalCase consts like `export const FooContext = createContext(...)`.
const COMPONENT_DECL_RE =
  /^export\s+(?:default\s+)?(?:const\s+([A-Z][A-Za-z0-9]*)\s*(?::[^=\n]+)?=\s*(?:<[^>]*>\s*)?\(|function\s+([A-Z][A-Za-z0-9]*)\s*[(<])/gm;
// Hook: `export const useX =` (any RHS) or `export function useX(`.
const HOOK_DECL_RE =
  /^export\s+(?:default\s+)?(?:const\s+(use[A-Z][A-Za-z0-9]*)\s*(?::[^=\n]+)?=|function\s+(use[A-Z][A-Za-z0-9]*)\s*[(<])/gm;

const namesFrom = (text, re) => {
  const names = new Set();
  for (const m of text.matchAll(re)) names.add(m[1] ?? m[2]);
  return [...names];
};

const inspectExports = (absolute, rel) => {
  if (!EXPORT_SCAN_RE.test(rel)) return [];
  if (isTestOrFixture(rel) || rel.endsWith('.d.ts')) return [];
  const isTsx = rel.endsWith('.tsx');
  if (!isTsx && !rel.endsWith('.ts')) return [];

  let text;
  try {
    text = readFileSync(absolute, 'utf8');
  } catch {
    return [];
  }

  const hooks = namesFrom(text, HOOK_DECL_RE);
  // Components only in .tsx; drop any use* that slipped past the component regex.
  const components = isTsx
    ? namesFrom(text, COMPONENT_DECL_RE).filter((n) => !n.startsWith('use'))
    : [];
  const units = [...components, ...hooks];
  if (units.length <= 1) return [];

  const noun = isTsx ? 'component or hook' : 'hook';
  return [
    `${rel}: exports ${units.length} React units (${units.join(', ')}) — one ${noun} per file. Split the extras into their own files; utility files (which export neither a component nor a hook) are exempt.`
  ];
};

const main = () => {
  const gitRoot = gitToplevel();
  const scan = ALL_MODE ? allFiles : changedFiles;
  const files = scan(gitRoot).filter((f) => f.startsWith(`${UI_ROOT}/`));

  const violations = [];
  for (const absolute of files) {
    if (!existsSync(absolute)) continue;
    const rel = relative(UI_ROOT, absolute);
    violations.push(...inspectPage(rel));
    if (absolute.endsWith('.tsx')) violations.push(...inspectComponent(absolute, rel));
    violations.push(...inspectExports(absolute, rel));
  }

  if (violations.length === 0) {
    if (!HOOK_MODE) process.stdout.write('check-structure: no violations\n');
    return 0;
  }

  const message = [
    'Folder-structure contract failed:',
    ...violations.map((v) => `- ${v}`),
    '',
    'Rules: a page folder holds only <X>Page.tsx + <X>Page.module.css + __tests__/.',
    'Every feature component lives in features/<feature>/components/<kebab>/ with a',
    'colocated .module.css and a __tests__/<Name>.test.tsx unit test.',
    'See operator-ui/CLAUDE.md ("File layout") and .claude/rules/folder-structure.md.'
  ].join('\n');

  if (HOOK_MODE) {
    process.stdout.write(JSON.stringify({ decision: 'block', reason: message }));
    return 0;
  }
  process.stderr.write(`${message}\n`);
  return 1;
};

// PostToolUse (Write|Edit) create-time check: inspect only the file just written,
// and only for rule 3 (one unit per file). The filesystem-shape rules (test
// existence, kebab folder) stay on Stop/CI where a whole-turn view is correct —
// blocking mid-write on a not-yet-created test would fight normal authoring.
const writeHookMain = () => {
  let filePath;
  try {
    const payload = JSON.parse(readFileSync(0, 'utf8'));
    filePath = payload?.tool_input?.file_path;
  } catch {
    return 0;
  }
  if (!filePath) return 0;
  const absolute = resolve(filePath);
  if (!absolute.startsWith(`${UI_ROOT}/`) || !existsSync(absolute)) return 0;

  const violations = inspectExports(absolute, relative(UI_ROOT, absolute));
  if (violations.length === 0) return 0;

  const message = [
    'Folder-structure contract failed (one React unit per file):',
    ...violations.map((v) => `- ${v}`),
    '',
    'A file exports at most one component (.tsx) or one use* hook. Move the extra',
    'unit into its own file; utility files (exporting neither) are exempt.',
    'See operator-ui/CLAUDE.md ("File layout") and .claude/rules/folder-structure.md.'
  ].join('\n');
  process.stdout.write(JSON.stringify({ decision: 'block', reason: message }));
  return 0;
};

process.exit(WRITE_HOOK_MODE ? writeHookMain() : main());
