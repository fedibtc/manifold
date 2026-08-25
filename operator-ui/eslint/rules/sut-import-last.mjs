/**
 * sut-import-last
 * In test files, the System Under Test must be the LAST import so mocks load
 * first (docs/clean-code.md §7). SUT heuristic: a relative import whose path
 * does not look like a helper/mock/fixture/util.
 */

const HELPER_PATH = /(mock|helper|fixture|test-util|testUtils|__mocks__|setup)/i;

const isSutPath = (value) =>
  (value.startsWith('./') || value.startsWith('../')) && !HELPER_PATH.test(value);

export default {
  meta: {
    type: 'suggestion',
    docs: { description: 'System Under Test must be the last import in test files' },
    schema: [],
    messages: {
      sutNotLast:
        "Import of '{{after}}' appears after the SUT import '{{sut}}'. The SUT must be imported LAST so mocks load first (docs/clean-code.md §7).",
    },
  },
  create(context) {
    const filename = context.filename ?? context.getFilename();
    if (!/\.(test|spec)\.[jt]sx?$/.test(filename)) return {};

    return {
      Program(program) {
        const imports = program.body.filter((n) => n.type === 'ImportDeclaration');
        let sut = null;
        for (const declaration of imports) {
          const path = declaration.source.value;
          if (sut && !isSutPath(path)) {
            context.report({
              node: declaration,
              messageId: 'sutNotLast',
              data: { after: path, sut },
            });
          }
          if (isSutPath(path)) sut = path;
        }
      },
    };
  },
};
