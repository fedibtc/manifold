#!/usr/bin/env node
/**
 * jsx-shape-dupes — detects repeated JSX layouts ("same layout with a hole").
 * Signature = child element names with text/attribute values stripped; one
 * wildcard position allowed. Signatures occurring >= 3 times (rule of three)
 * are reported. Report-only by default; --fail exits 1 on findings.
 *
 * Usage: node jsx-shape-dupes.mjs [--fail] [--min 3] [dir=src]
 * Requires: @babel/parser
 */
import { parse } from '@babel/parser';
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, extname } from 'node:path';

const args = process.argv.slice(2);
const shouldFail = args.includes('--fail');
const minIndex = args.indexOf('--min');
const MIN_OCCURRENCES = minIndex !== -1 ? Number(args[minIndex + 1]) : 3;
const roots = args.filter((a, i) => !a.startsWith('--') && args[i - 1] !== '--min');
const ROOTS = roots.length > 0 ? roots : ['src'];

const IGNORED_DIRS = /(node_modules|\.next|dist|build|coverage|\.git)/;
const JSX_EXTENSIONS = new Set(['.jsx', '.tsx']);
const MIN_CHILDREN = 3;

const collectFiles = (dir, out = []) => {
  let entries;
  try {
    entries = readdirSync(dir);
  } catch {
    return out;
  }
  for (const entry of entries) {
    const full = join(dir, entry);
    if (IGNORED_DIRS.test(full)) continue;
    const stats = statSync(full);
    if (stats.isDirectory()) collectFiles(full, out);
    else if (JSX_EXTENSIONS.has(extname(full))) out.push(full);
  }
  return out;
};

const elementName = (node) => {
  if (node.type === 'JSXElement') {
    const { name } = node.openingElement;
    if (name.type === 'JSXIdentifier') return name.name;
    if (name.type === 'JSXMemberExpression') return `${name.object.name}.${name.property.name}`;
  }
  if (node.type === 'JSXFragment') return '<>';
  if (node.type === 'JSXExpressionContainer') return '{expr}';
  return null;
};

const childSignature = (node) => {
  const children = (node.children || [])
    .map(elementName)
    .filter(Boolean);
  return children.length >= MIN_CHILDREN ? children : null;
};

/** Yield the exact signature plus each one-wildcard variant. */
const signatureVariants = (names) => {
  const variants = [names.join(',')];
  for (let i = 0; i < names.length; i++) {
    const copy = [...names];
    copy[i] = '*';
    variants.push(copy.join(','));
  }
  return variants;
};

const walk = (node, visit) => {
  if (!node || typeof node.type !== 'string') return;
  visit(node);
  for (const key of Object.keys(node)) {
    if (key === 'loc' || key === 'range' || key === 'parent') continue;
    const value = node[key];
    if (Array.isArray(value)) value.forEach((child) => walk(child, visit));
    else if (value && typeof value.type === 'string') walk(value, visit);
  }
};

const occurrencesBySignature = new Map();
const files = ROOTS.flatMap((root) => collectFiles(root));

for (const file of files) {
  let ast;
  try {
    ast = parse(readFileSync(file, 'utf8'), {
      sourceType: 'module',
      plugins: ['jsx', 'typescript'],
    });
  } catch {
    continue;
  }
  walk(ast.program, (node) => {
    if (node.type !== 'JSXElement' && node.type !== 'JSXFragment') return;
    const names = childSignature(node);
    if (!names) return;
    const location = `${file}:${node.loc.start.line}`;
    for (const variant of signatureVariants(names)) {
      if (!occurrencesBySignature.has(variant)) occurrencesBySignature.set(variant, new Set());
      occurrencesBySignature.get(variant).add(location);
    }
  });
}

// Prefer exact signatures; report wildcard variants only when no exact parent
// signature already covers the same locations.
const findings = [];
const reportedLocations = new Set();

const sorted = [...occurrencesBySignature.entries()]
  .filter(([, locations]) => locations.size >= MIN_OCCURRENCES)
  .sort(([a], [b]) => (a.includes('*') ? 1 : 0) - (b.includes('*') ? 1 : 0));

for (const [signature, locations] of sorted) {
  const fresh = [...locations].filter((l) => !reportedLocations.has(l));
  if (fresh.length < MIN_OCCURRENCES) continue;
  fresh.forEach((l) => reportedLocations.add(l));
  findings.push({ signature, locations: [...locations] });
}

if (findings.length === 0) {
  console.log('jsx-shape-dupes: no repeated layouts found.');
  process.exit(0);
}

for (const { signature, locations } of findings) {
  console.log(`\nRepeated layout [${signature}] × ${locations.length}:`);
  locations.forEach((location) => console.log(`  ${location}`));
  console.log(
    '  → repeated layout — consider extracting a domain-named component with a children slot for the varying element (rule of three, docs/clean-code.md §4).'
  );
}

process.exit(shouldFail ? 1 : 0);
