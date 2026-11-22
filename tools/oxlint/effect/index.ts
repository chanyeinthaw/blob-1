import { definePlugin } from '@oxlint/plugins'

import noGlobalProcessRuntime from './rules/no-global-process-runtime.ts'
import noInlineSchemaCompile from './rules/no-inline-schema-compile.ts'
import noManualEffectRuntimeInTests from './rules/no-manual-effect-runtime-in-tests.ts'
import preferCatchTags from './rules/prefer-catch-tags.ts'

export default definePlugin({
  meta: {
    name: 'effect-local',
  },
  rules: {
    'no-global-process-runtime': noGlobalProcessRuntime,
    'no-inline-schema-compile': noInlineSchemaCompile,
    'no-manual-effect-runtime-in-tests': noManualEffectRuntimeInTests,
    'prefer-catch-tags': preferCatchTags,
  },
})
