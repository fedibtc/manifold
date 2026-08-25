#!/usr/bin/env node
// operator-ui external-CSS style checker (Tailwind v3 + CSS Modules).
//
// Enforces the one load-bearing rule nothing else catches: NO Tailwind utility
// strings in TSX. `className` may hold only `styles.*` refs or a merged caller
// `className` — no string literals (inline, braced, ternary) and no style
// consts. Also rejects v4-only directives (@reference/@theme/@utility) that do
// not belong in this v3 codebase.
//
// Modes:
//   node scripts/check-styles.mjs          gate mode: human output, exit 1 on
//                                           violations, 0 when clean.
//   node scripts/check-styles.mjs --hook    Claude Code Stop hook: emit
//                                           {"decision":"block",...} JSON on
//                                           violations, always exit 0.
//
// Path handling: git reports paths relative to the repo root (operator-ui is a
// subdir of the decentralized-federations monorepo). We resolve against the git
// toplevel, then keep only files under this script's operator-ui root — so the
// checker is correct whether invoked from the monorepo root or operator-ui.

import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { dirname, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const HOOK_MODE = process.argv.includes('--hook');
const UI_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const SHARED_UTILITY_FILE_RE = /(^|\/)apps\/[^/]+\/src\/shared\/styles\/utilities\.css$/;
const SHARED_UTILITY_NAMES = new Set([
  'stack1',
  'stack2',
  'stack3',
  'stack4',
  'stack6',
  'stack8',
  'row2',
  'wrapRow2',
  'wrapRow3',
  'listSm',
  'pageHeading',
  'mutedSm',
  'mutedXs',
  'mutedXsFaint',
  'inkLabel',
  'label',
  'subSm',
  'sectionHeading',
  'noteXs',
  'errorLabel',
  'subText',
  'intro',
  'centeredPage',
  'tableWrap',
  'modalCard',
  'confirmDanger',
  'emptyStateSm',
  'removeBtn'
]);

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

const isExemptTsx = (file) =>
  /\.(test|spec|stories)\.tsx$/i.test(file) ||
  /(^|\/)(__tests__|__fixtures__|generated)(\/|$)/i.test(file);

// Strip line and block comments so commented examples don't trip the checker.
const stripComments = (src) => src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/[^\n]*/g, '');

// Find every `className={...}` / `className="..."` expression and return the
// raw expression text (the attribute value) for each.
const classNameExprs = (src) => {
  const exprs = [];
  const re = /className\s*=\s*/g;
  for (const match of src.matchAll(re)) {
    const i = match.index + match[0].length;
    const ch = src[i];
    if (ch === '"' || ch === "'" || ch === '`') {
      // Direct string-literal attribute: className="...".
      const end = src.indexOf(ch, i + 1);
      exprs.push({ raw: src.slice(i, end < 0 ? src.length : end + 1), literal: true });
    } else if (ch === '{') {
      // Braced expression: scan to the matching close brace.
      let depth = 0;
      let j = i;
      for (; j < src.length; j += 1) {
        if (src[j] === '{') depth += 1;
        else if (src[j] === '}') {
          depth -= 1;
          if (depth === 0) break;
        }
      }
      exprs.push({ raw: src.slice(i + 1, j), literal: false });
    }
  }
  return exprs;
};

const hasStringLiteral = (expr) => /["'`]/.test(expr);
const isSharedUtilityFile = (file) => SHARED_UTILITY_FILE_RE.test(file);

// Bare identifiers used as `className={IDENT}` (not member access, not a call).
const bareClassNameIdents = (src) => {
  const idents = new Set();
  const re = /className\s*=\s*\{\s*([A-Za-z_$][\w$]*)\s*\}/g;
  for (const match of src.matchAll(re)) idents.add(match[1]);
  return idents;
};

const inspectTsx = (absolute, rel) => {
  const violations = [];
  const raw = readFileSync(absolute, 'utf8');
  const src = stripComments(raw);

  for (const { raw: expr, literal } of classNameExprs(src)) {
    if (literal) {
      violations.push(
        `${rel}: Tailwind string in className — move it into a CSS module (styles.*)`
      );
    } else if (hasStringLiteral(expr)) {
      violations.push(
        `${rel}: string literal inside className={...} — use styles.* refs only (no inline utilities, no ternary strings)`
      );
    }
  }

  // Style consts: a bare identifier in className that resolves to a string literal.
  const idents = bareClassNameIdents(src);
  for (const ident of idents) {
    const constRe = new RegExp(String.raw`\b(?:const|let|var)\s+${ident}\s*=\s*["'\`]`);
    if (constRe.test(src)) {
      violations.push(
        `${rel}: style const "${ident}" — no utility strings in consts; put styling in the CSS module`
      );
    }
  }

  return violations;
};

const inspectCssModule = (absolute, rel) => {
  const violations = [];
  const src = stripComments(readFileSync(absolute, 'utf8'));

  // v3 CSS Modules are processed in isolation with no `@tailwind` context, so
  // `@layer components` fails to build ("no matching @tailwind components").
  // Modules must use bare `@apply`. Reject the wrapper and all v4-only directives.
  if (/@layer\b/.test(src)) {
    violations.push(
      `${rel}: remove @layer — v3 CSS modules use bare @apply (no @layer; it needs a @tailwind context the module lacks)`
    );
  }
  if (/@reference\b/.test(src)) {
    violations.push(`${rel}: remove @reference (v4-only; this repo is Tailwind v3)`);
  }
  if (/@theme\b/.test(src)) {
    violations.push(`${rel}: move @theme tokens to the shared preset (tailwind-preset.cjs)`);
  }
  if (/@utility\b/.test(src)) {
    violations.push(
      `${rel}: move @utility to a shared @layer utilities file (v4 @utility is not used here)`
    );
  }

  // A class that @applies its OWN name as a complete utility token creates a
  // circular dependency at build time (e.g. `.grid { @apply grid }`). Match the
  // exact whitespace-delimited token (minus any variant prefix like `md:`), not
  // a substring — `.card { @apply rounded-card }` is fine.
  const ruleRe = /\.([a-zA-Z][\w-]*)\s*\{([^}]*)\}/g;
  for (const rule of src.matchAll(ruleRe)) {
    const [, name, body] = rule;
    const applyRe = /@apply\s+([^;]+);/g;
    for (const apply of body.matchAll(applyRe)) {
      const tokens = apply[1].split(/\s+/).map((t) => t.split(':').at(-1));
      const legacySharedUtility = tokens.find((token) => SHARED_UTILITY_NAMES.has(token));
      if (legacySharedUtility) {
        violations.push(
          `${rel}: @apply uses shared utility "${legacySharedUtility}" without the required "u" prefix — use "u${legacySharedUtility[0].toUpperCase()}${legacySharedUtility.slice(1)}"`
        );
      }
      if (tokens.includes(name)) {
        violations.push(
          `${rel}: class ".${name}" @applies the "${name}" utility — rename the class (circular dependency at build)`
        );
        break;
      }
    }
  }

  return violations;
};

const inspectSharedUtilityCss = (absolute, rel) => {
  const violations = [];
  const src = stripComments(readFileSync(absolute, 'utf8'));
  const ruleRe = /\.([a-zA-Z][\w-]*)\s*\{/g;
  for (const rule of src.matchAll(ruleRe)) {
    const [, name] = rule;
    if (!name.startsWith('u')) {
      violations.push(
        `${rel}: shared utility ".${name}" must use the "u" prefix (for example ".u${name[0].toUpperCase()}${name.slice(1)}")`
      );
    }
  }
  return violations;
};

const main = () => {
  const gitRoot = gitToplevel();
  const files = changedFiles(gitRoot).filter(
    (f) =>
      (f.endsWith('.tsx') || f.endsWith('.module.css') || isSharedUtilityFile(f)) &&
      f.startsWith(`${UI_ROOT}/`)
  );

  const violations = [];
  for (const absolute of files) {
    if (!existsSync(absolute)) continue;
    const rel = relative(UI_ROOT, absolute);
    if (absolute.endsWith('.tsx')) {
      if (isExemptTsx(absolute)) continue;
      violations.push(...inspectTsx(absolute, rel));
    } else if (isSharedUtilityFile(absolute)) {
      violations.push(...inspectSharedUtilityCss(absolute, rel));
    } else {
      violations.push(...inspectCssModule(absolute, rel));
    }
  }

  if (violations.length === 0) {
    if (!HOOK_MODE) process.stdout.write('check-styles: no violations\n');
    return 0;
  }

  const message = [
    'External-CSS style contract failed:',
    ...violations.map((v) => `- ${v}`),
    '',
    'Rule: className holds only styles.* refs (or a merged caller className).',
    'No utility strings in TSX — not inline, not in a ternary, not in a const.',
    'See .claude/rules/tailwind-css.md; use the tailwind-component-styles skill.'
  ].join('\n');

  if (HOOK_MODE) {
    process.stdout.write(JSON.stringify({ decision: 'block', reason: message }));
    return 0;
  }
  process.stderr.write(`${message}\n`);
  return 1;
};

process.exit(main());
