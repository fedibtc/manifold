#!/usr/bin/env node
/**
 * ratchet — quality metrics may only improve.
 * Tracks: lintWarnings (eslint warning count), typeCoverage (%).
 * `check` fails if a metric regressed vs the committed baseline;
 * `update` rewrites the baseline (run after intentional improvements).
 */
import { execSync } from 'node:child_process';
import { readFileSync, writeFileSync, existsSync } from 'node:fs';

const mode = process.argv[2] || 'check';
const config = JSON.parse(readFileSync('harness.config.json', 'utf8'));
const baselineFile = config.ratchet?.baselineFile || '.quality-baseline.json';
const tracked = config.ratchet?.track || ['lintWarnings', 'typeCoverage'];

const sh = (cmd) => {
  try {
    return execSync(cmd, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] });
  } catch (error) {
    return error.stdout || '';
  }
};

const measure = {
  lintWarnings: () => {
    const raw = sh('npx --no-install eslint . --format json --no-error-on-unmatched-pattern');
    if (!raw.trim()) return null;
    try {
      return JSON.parse(raw).reduce((sum, file) => sum + file.warningCount, 0);
    } catch {
      return null;
    }
  },
  typeCoverage: () => {
    const raw = sh('npx --no-install type-coverage');
    const match = raw.match(/([\d.]+)%/);
    return match ? Number(match[1]) : null;
  },
};

// Higher is better for coverage; lower is better for warnings.
const isRegression = { lintWarnings: (now, base) => now > base, typeCoverage: (now, base) => now < base };

const current = {};
for (const metric of tracked) {
  const value = measure[metric]?.();
  if (value !== null && value !== undefined) current[metric] = value;
}

if (mode === 'update' || !existsSync(baselineFile)) {
  writeFileSync(baselineFile, JSON.stringify(current, null, 2) + '\n');
  console.log(`ratchet: baseline ${mode === 'update' ? 'updated' : 'created'} → ${baselineFile}`, current);
  process.exit(0);
}

const baseline = JSON.parse(readFileSync(baselineFile, 'utf8'));
const regressions = [];
const improvements = [];

for (const [metric, value] of Object.entries(current)) {
  if (!(metric in baseline)) continue;
  if (isRegression[metric](value, baseline[metric])) {
    regressions.push(`${metric}: ${baseline[metric]} → ${value} (regressed)`);
  } else if (value !== baseline[metric]) {
    improvements.push(`${metric}: ${baseline[metric]} → ${value}`);
  }
}

if (improvements.length > 0) {
  console.log('ratchet: improvements detected — run `node scripts/ratchet.mjs update` to lock them in:');
  improvements.forEach((line) => console.log(`  ${line}`));
}

if (regressions.length > 0) {
  console.error('ratchet: quality regressed — fix before committing:');
  regressions.forEach((line) => console.error(`  ${line}`));
  process.exit(2);
}

console.log('ratchet: no regressions.');
