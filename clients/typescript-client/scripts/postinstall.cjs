#!/usr/bin/env node
/**
 * Opt-in postinstall hook for @zlayer/client.
 *
 * By default this script is a no-op — we do not download the zlayer
 * binary during `npm install`, because that's a surprise network
 * activity (and would also fail on CI / offline installs / Windows).
 *
 * Users who *do* want the binary auto-downloaded can set the
 * `ZLAYER_AUTO_INSTALL=1` environment variable before installing:
 *
 *     ZLAYER_AUTO_INSTALL=1 npm install @zlayer/client
 *
 * Any failure is swallowed with a warning — an `npm install` never
 * fails just because we could not reach GitHub.
 */
"use strict";

if (process.env.ZLAYER_AUTO_INSTALL !== "1") {
  // Silent no-op in the normal case.
  process.exit(0);
}

(async function main() {
  let ensureDaemon;
  try {
    // Require the built output. On fresh npm installs the `dist/` folder
    // is published alongside the package, so this path works without a
    // separate build step at install time.
    ({ ensureDaemon } = require("../dist/install.js"));
  } catch (err) {
    console.warn(
      "[@zlayer/client] postinstall: dist/install.js not found (run `npm run build` first).",
      err && err.message ? err.message : err,
    );
    process.exit(0);
  }

  try {
    const path = await ensureDaemon();
    console.log(`[@zlayer/client] installed zlayer binary to ${path}`);
  } catch (err) {
    console.warn(
      "[@zlayer/client] postinstall: could not download zlayer binary " +
        "(set ZLAYER_AUTO_INSTALL=0 to disable this warning).",
    );
    console.warn(
      "  Reason:",
      err && err.message ? err.message : err,
    );
    // Never fail `npm install`.
    process.exit(0);
  }
})();
