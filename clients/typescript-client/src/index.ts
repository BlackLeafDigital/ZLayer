/**
 * @zlayer/client — ergonomic TypeScript SDK for the ZLayer daemon.
 *
 * ```ts
 * import { Client, ensureDaemon } from "@zlayer/client";
 *
 * await ensureDaemon();               // download binary to ~/.local/share/zlayer/bin/
 * const client = new Client();        // defaults to the platform Unix socket
 * console.log(await client.ps());
 * ```
 *
 * For full access to the auto-generated API classes, use
 * {@link Client.raw} or import directly from `@zlayer/api-client`.
 */
export {
  Client,
  type BuildOptions,
  type ClientOptions,
  type ExecResult,
  type LogsOptions,
  type RawApis,
} from "./client.js";

export {
  buildTransport,
  defaultSocketPath,
  type FetchLike,
  type TransportOptions,
} from "./transport.js";

export {
  ensureDaemon,
  defaultBinaryDir,
  ZLayerInstallError,
  type EnsureDaemonOptions,
} from "./install.js";

// Re-export key model types so callers can type their results without
// also depending on `@zlayer/api-client` directly.
export type {
  BuildStatus,
  ContainerInfo,
  DeploymentDetails,
  DeploymentSummary,
  PullImageResponse,
  ServiceDetails,
  ServiceSummary,
  TemplateInfo,
  TriggerBuildResponse,
} from "@zlayer/api-client";
