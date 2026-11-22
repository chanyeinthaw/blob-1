import { definePlugin } from "@oxlint/plugins";

import noTestInLoop from "./rules/no-test-in-loop.ts";
import requireSuppressionReason from "./rules/require-suppression-reason.ts";

/** General hygiene rules that apply across the workspace. */
export default definePlugin({
  meta: {
    name: "hygiene",
  },
  rules: {
    "no-test-in-loop": noTestInLoop,
    "require-suppression-reason": requireSuppressionReason,
  },
});
