import { defineRule } from '@oxlint/plugins'

const RAW_CLIENT = /(?:^|\/)rpc\/client(?:\.ts)?$/u

/**
 * The workspace socket is an implementation detail of the state layer. React
 * code reads atoms, so a component can never hold a half-authenticated call.
 */
export default defineRule({
  meta: {
    type: 'problem',
    docs: {
      description: 'Import the raw workspace RPC client only from the state layer.',
    },
  },
  create(context) {
    return {
      ImportDeclaration(node) {
        if (node.importKind === 'type') return
        if (typeof node.source.value !== 'string') return
        if (!RAW_CLIENT.test(node.source.value)) return
        context.report({
          node,
          message:
            'Read the workspace through state atoms. The raw RPC client belongs in src/state, next to the atoms that own it.',
        })
      },
    }
  },
})
