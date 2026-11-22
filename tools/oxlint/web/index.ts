import { definePlugin } from "@oxlint/plugins";

import noNativeTitleTooltip from "./rules/no-native-title-tooltip.ts";
import noRawResizeObserver from "./rules/no-raw-resize-observer.ts";
import noRawRpcClient from "./rules/no-raw-rpc-client.ts";
import noOutsetStateIndicators from "./rules/no-outset-state-indicators.ts";
import noUnscopedHas from "./rules/no-unscoped-has.ts";
import requireCenteredScrollGutter from "./rules/require-centered-scroll-gutter.ts";

/** Rules for the React and Tailwind layers of the web client. */
export default definePlugin({
  meta: {
    name: "web",
  },
  rules: {
    "no-native-title-tooltip": noNativeTitleTooltip,
    "no-raw-resize-observer": noRawResizeObserver,
    "no-raw-rpc-client": noRawRpcClient,
    "no-outset-state-indicators": noOutsetStateIndicators,
    "no-unscoped-has": noUnscopedHas,
    "require-centered-scroll-gutter": requireCenteredScrollGutter,
  },
});
