/**
 * max-usestate-per-component
 * Caps useState calls per component (default 4). More than that usually means
 * the component owns state that belongs lower, or needs a custom hook / reducer.
 */

const DEFAULT_MAX = 4;

const isFunction = (node) =>
  node.type === 'FunctionDeclaration' ||
  node.type === 'FunctionExpression' ||
  node.type === 'ArrowFunctionExpression';

export default {
  meta: {
    type: 'suggestion',
    docs: { description: 'Cap useState calls per component' },
    schema: [
      {
        type: 'object',
        properties: { max: { type: 'integer', minimum: 1 } },
        additionalProperties: false,
      },
    ],
    messages: {
      tooMany:
        '{{count}} useState calls in one component (max {{max}}). Extract a custom hook, use useReducer, or colocate state lower (docs/clean-code.md §2, §6).',
    },
  },
  create(context) {
    const max = (context.options[0] && context.options[0].max) || DEFAULT_MAX;
    const stack = [];

    const enter = (node) => stack.push({ node, count: 0 });
    const exit = () => {
      const frame = stack.pop();
      if (frame && frame.count > max) {
        context.report({
          node: frame.node,
          messageId: 'tooMany',
          data: { count: frame.count, max },
        });
      }
    };

    return {
      FunctionDeclaration: enter,
      FunctionExpression: enter,
      ArrowFunctionExpression: enter,
      'FunctionDeclaration:exit': exit,
      'FunctionExpression:exit': exit,
      'ArrowFunctionExpression:exit': exit,
      CallExpression(node) {
        const isUseState =
          (node.callee.type === 'Identifier' && node.callee.name === 'useState') ||
          (node.callee.type === 'MemberExpression' &&
            node.callee.property.type === 'Identifier' &&
            node.callee.property.name === 'useState');
        if (isUseState && stack.length > 0) stack[stack.length - 1].count += 1;
      },
    };
  },
};
