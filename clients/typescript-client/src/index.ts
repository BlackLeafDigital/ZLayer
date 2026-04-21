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
  type ConnectBridgeNetworkInput,
  type ContainerKillOptions,
  type ContainerStopOptions,
  type CreateBridgeNetworkInput,
  type CreateContainerInput,
  type CreateRegistryCredentialInput,
  type CreateVolumeInput,
  type DeleteBridgeNetworkOptions,
  type DeleteContainerOptions,
  type DeleteVolumeOptions,
  type DisconnectBridgeNetworkOptions,
  type EventsStreamOptions,
  type ExecResult,
  type ExecStreamEvent,
  type ExecStreamOptions,
  type ListBridgeNetworksOptions,
  type ListContainersOptions,
  type LogsOptions,
  type PullImageInput,
  type RawApis,
  type StatsStreamOptions,
} from "./client.js";

export {
  buildTransport,
  defaultSocketPath,
  type FetchLike,
  type TransportOptions,
} from "./transport.js";

export {
  consumeSseStream,
  type SseFrame,
} from "./sse.js";

export {
  ensureDaemon,
  defaultBinaryDir,
  ZLayerInstallError,
  type EnsureDaemonOptions,
} from "./install.js";

// Re-export key model types so callers can type their results without
// also depending on `@zlayer/api-client` directly.
export type {
  BridgeNetwork,
  BridgeNetworkAttachment,
  BridgeNetworkDetails,
  BridgeNetworkDriver,
  BuildStatus,
  ConnectBridgeNetworkRequest,
  ContainerEvent,
  ContainerEventKind,
  ContainerInfo,
  ContainerRestartKind,
  ContainerRestartPolicy,
  ContainerStatsResponse,
  ContainerWaitResponse,
  CreateBridgeNetworkRequest,
  CreateContainerRequest,
  CreateRegistryCredentialRequest,
  CreateVolumeRequest,
  DeploymentDetails,
  DeploymentSummary,
  DisconnectBridgeNetworkRequest,
  HealthCheckRequest,
  NetworkAttachmentRequest,
  PortMapping,
  PullImageRequest,
  PullImageResponse,
  RegistryAuth,
  RegistryAuthTypeSchema,
  RegistryCredentialResponse,
  ServiceDetails,
  ServiceSummary,
  TemplateInfo,
  TriggerBuildResponse,
  VolumeInfo,
} from "@zlayer/api-client";
