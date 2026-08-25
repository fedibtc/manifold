/**
 * Feature-based structure boundaries — docs/clean-code.md §10.
 * One-way import flow: shared ← features ← app. No cross-feature imports.
 *
 * Paths and severity come from harness.config.json → boundaries.
 * Degrades to [] (no-op) when eslint-plugin-boundaries is not installed or
 * boundaries.enabled is false, so base.config.mjs can always spread it.
 */
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { join } from 'node:path';

const require = createRequire(import.meta.url);

const DEFAULTS = {
  enabled: true,
  severity: 'warn',
  shared: 'src/shared',
  features: 'src/features/*',
  app: 'src/app',
};

const readBoundariesConfig = () => {
  try {
    const raw = readFileSync(join(process.cwd(), 'harness.config.json'), 'utf8');
    return JSON.parse(raw).boundaries ?? {};
  } catch {
    return {};
  }
};

const loadPlugin = () => {
  try {
    return require('eslint-plugin-boundaries');
  } catch {
    return null; // installer adds the dep; until then this fragment is a no-op
  }
};

const toArray = (value) => (Array.isArray(value) ? value : [value]);

export const buildBoundariesConfig = (overrides = readBoundariesConfig()) => {
  const boundaries = { ...DEFAULTS, ...overrides };
  const plugin = loadPlugin();
  if (!boundaries.enabled || !plugin) return [];

  const sharedPatterns = toArray(boundaries.shared).map((path) => `${path}/**/*`);
  const appPatterns = toArray(boundaries.app).map((path) => `${path}/**/*`);
  const featurePattern = `${boundaries.features}/**/*`;

  return [
    {
      files: ['**/*.{ts,tsx,js,jsx}'],
      plugins: { boundaries: plugin },
      settings: {
        'boundaries/elements': [
          { type: 'shared', mode: 'full', pattern: sharedPatterns },
          { type: 'feature', mode: 'full', pattern: [featurePattern], capture: ['featureName'] },
          { type: 'app', mode: 'full', pattern: appPatterns },
        ],
      },
      rules: {
        'boundaries/element-types': [
          boundaries.severity,
          {
            default: 'disallow',
            message:
              '${file.type} code must not import ${dependency.type} code — one-way flow is shared ← features ← app (docs/clean-code.md §10)',
            rules: [
              { from: ['shared'], allow: ['shared'] },
              {
                from: ['feature'],
                allow: ['shared', ['feature', { featureName: '${from.featureName}' }]],
                message:
                  'Features import only from shared/ or themselves — never another feature. Shared by 2+ features? Move it to shared/ (docs/clean-code.md §10)',
              },
              { from: ['app'], allow: ['shared', 'feature', 'app'] },
            ],
          },
        ],
      },
    },
  ];
};

export default buildBoundariesConfig();
