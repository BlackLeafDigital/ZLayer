/**
 * Ergonomic ZLayer client.
 *
 * Mirrors the Python `zlayer.Client` API:
 *
 * ```ts
 * import { Client } from "@zlayer/client";
 *
 * const client = new Client();                 // defaults to the Unix socket
 * console.log(await client.ps());              // list running containers
 * await client.deploy(specYaml);               // deploy a spec
 * const out = await client.logs("containerId"); // get logs
 * ```
 *
 * All methods are thin wrappers over the auto-generated `@zlayer/api-client`
 * classes and return the same model types. The wrapper renames methods to
 * match the Python SDK (and the `zlayer` CLI verbs) and hides the
 * request-wrapper objects (`{createDeploymentRequest: {spec}}`) behind
 * positional arguments.
 */
import {
  BuildApi,
  Configuration,
  ContainerNetworksApi,
  ContainersApi,
  CredentialsApi,
  DeploymentsApi,
  EventsApi,
  ImagesApi,
  ServicesApi,
  VolumesApi,
  type BridgeNetwork,
  type BridgeNetworkDetails,
  type BridgeNetworkDriver,
  type BuildStatus,
  type ContainerEvent,
  type ContainerInfo,
  type ContainerStatsResponse,
  type ContainerWaitResponse,
  type CreateContainerRequest,
  type DeploymentDetails,
  type DeploymentSummary,
  type PullImageResponse,
  type RegistryAuthTypeSchema,
  type RegistryCredentialResponse,
  type ServiceDetails,
  type ServiceSummary,
  type TemplateInfo,
  type TriggerBuildResponse,
  type VolumeInfo,
  ContainerEventFromJSON,
  ContainerStatsResponseFromJSON,
} from "@zlayer/api-client";

import { consumeSseStream } from "./sse.js";
import { buildTransport, type FetchLike, type TransportOptions } from "./transport.js";

/** Options for constructing a {@link Client}. */
export interface ClientOptions extends TransportOptions {
  /**
   * Bearer token to send on every request (maps to the `accessToken`
   * field on the generated `Configuration`). When omitted, auth headers
   * are not set — appropriate for a local daemon talking over the
   * trusted Unix socket.
   */
  token?: string;

  /**
   * Override the `fetch` implementation. Mostly useful for tests — the
   * default auto-selects between a Unix-socket fetch and the built-in
   * fetch based on {@link TransportOptions}.
   */
  fetchApi?: FetchLike;
}

/** Optional filters for {@link Client.logs}. */
export interface LogsOptions {
  /** Number of lines from the tail of the log to return. */
  tail?: number;
  /**
   * Whether to follow the log stream. The generated client returns the
   * response body as a single string, so for true streaming callers
   * should consume `raw.containers.getContainerLogsRaw()` and read from
   * `response.raw.body`.
   */
  follow?: boolean;
}

/** Optional fields for {@link Client.build}. */
export interface BuildOptions {
  /** Build arguments (`--build-arg KEY=VALUE`). */
  buildArgs?: Record<string, string>;
  /** Disable the cache for this build. */
  noCache?: boolean;
  /** Push the image after a successful build. */
  push?: boolean;
  /** Use a runtime template (e.g. `node`, `python`) instead of a Dockerfile. */
  runtime?: string;
  /** Dockerfile target stage. */
  target?: string;
}

/**
 * Raw escape hatches. If the façade is missing a method or you need
 * fine-grained access to the generated request wrappers, use these.
 */
export interface RawApis {
  build: BuildApi;
  containers: ContainersApi;
  containerNetworks: ContainerNetworksApi;
  credentials: CredentialsApi;
  deployments: DeploymentsApi;
  events: EventsApi;
  images: ImagesApi;
  services: ServicesApi;
  volumes: VolumesApi;
  configuration: Configuration;
}

/** Options accepted by {@link Client.stopContainer} / {@link Client.restartContainer}. */
export interface ContainerStopOptions {
  /**
   * Graceful shutdown timeout in seconds before the runtime force-kills the
   * container. When omitted the daemon's default (30 seconds) applies.
   */
  timeoutSeconds?: number;
}

/** Options accepted by {@link Client.killContainer}. */
export interface ContainerKillOptions {
  /**
   * Signal name to send (e.g. `"SIGTERM"`, `"SIGINT"`). Accepts both the
   * `SIG`-prefixed and bare forms. When omitted the daemon defaults to
   * `SIGKILL`.
   */
  signal?: string;
}

/** Body for {@link Client.createRegistryCredential}. */
export interface CreateRegistryCredentialInput {
  /** Registry hostname (e.g. `"docker.io"`, `"ghcr.io"`). */
  registry: string;
  /** Username for authentication. */
  username: string;
  /** Password or token value (stored encrypted, never returned). */
  password: string;
  /** Authentication method. Defaults to `"basic"` when omitted. */
  authType?: RegistryAuthTypeSchema;
}

/** Body for {@link Client.pullImage}. */
export interface PullImageInput {
  /** OCI image reference to pull (e.g. `docker.io/library/nginx:latest`). */
  reference: string;
  /**
   * Pull policy override. Defaults to `"always"` on the server when omitted,
   * matching Docker-compat `POST /images/create` semantics.
   */
  pullPolicy?: "always" | "if_not_present" | "never";
}

/**
 * Ergonomic variant of the generated `CreateContainerRequest`.
 *
 * Exposed as an alias so the public surface reads naturally —
 * `client.createContainer(req)` takes a `CreateContainerInput` whose
 * shape matches the wire body one-for-one. Every optional field from
 * the generated type is passed through (ports, networks, health check,
 * restart policy, hostname, dns, extra hosts, volumes with the `type`
 * discriminator, inline or persisted registry auth, resources, etc.).
 */
export type CreateContainerInput = CreateContainerRequest;

/** Options accepted by {@link Client.deleteContainer}. */
export interface DeleteContainerOptions {
  /**
   * When true, force-remove even if the container is running. Matches the
   * Docker-compat `?force=true` query knob on `DELETE /containers/{id}`.
   * When the server does not yet honor the flag, this option is silently
   * ignored (the generated `deleteContainer` path takes no force query).
   */
  force?: boolean;
}

/** Options accepted by {@link Client.listContainers}. */
export interface ListContainersOptions {
  /**
   * Filter the returned list by label. Each entry is sent as a
   * `label=k=v` pair; passing multiple entries applies AND semantics
   * on the server. Equivalent to Docker's `-f label=k=v`.
   */
  label?: Record<string, string>;
}

/** Options accepted by {@link Client.execStream}. */
export interface ExecStreamOptions {
  /** argv of the command to run. Required; must be non-empty. */
  command: string[];
  /**
   * Extra environment variables layered onto the container's existing
   * env. Reserved for forward-compatibility — the current daemon
   * deserializer silently ignores unknown fields, so it's safe to pass
   * today.
   */
  env?: Record<string, string>;
  /** Request a PTY-attached exec. Also forward-compatible (see above). */
  tty?: boolean;
}

/** One SSE frame yielded by {@link Client.execStream}. */
export type ExecStreamEvent =
  | { kind: "stdout"; data: string }
  | { kind: "stderr"; data: string }
  | { kind: "exit"; exitCode: number };

/** Options accepted by {@link Client.streamStats}. */
export interface StatsStreamOptions {
  /**
   * Sampling interval in seconds between emitted frames. When omitted
   * the daemon applies its default cadence.
   */
  intervalSeconds?: number;
}

/** Options accepted by {@link Client.streamEvents}. */
export interface EventsStreamOptions {
  /**
   * Filter by container labels. Each `k=v` pair becomes one `label`
   * query-string entry (AND semantics server-side).
   */
  labels?: Record<string, string>;
}

/** Options accepted by {@link Client.createVolume}. */
export interface CreateVolumeInput {
  /** Volume name. Must match `^[a-z0-9][a-z0-9_-]{0,63}$`. */
  name: string;
  /** Size hint in humansize format (`"512Mi"`, `"10Gi"`). Not enforced. */
  size?: string;
  /** Storage tier: `"local"` (default), `"cached"`, `"network"`. */
  tier?: string;
  /** Labels attached to the volume. */
  labels?: Record<string, string>;
}

/** Options accepted by {@link Client.deleteVolume}. */
export interface DeleteVolumeOptions {
  /**
   * When true, bypass the "non-empty or in-use" safety check and remove
   * anyway. Maps to `DELETE /api/v1/volumes/{name}?force=true`.
   */
  force?: boolean;
}

/** Options accepted by {@link Client.createBridgeNetwork}. */
export interface CreateBridgeNetworkInput {
  /** Network name. Must match `^[a-z0-9][a-z0-9_-]{0,63}$`. */
  name: string;
  /** Driver, defaults server-side to `"bridge"`. */
  driver?: BridgeNetworkDriver;
  /** Subnet CIDR (e.g. `"10.240.0.0/24"`). */
  subnet?: string;
  /** Labels for filtering and grouping. */
  labels?: Record<string, string>;
  /** Internal-only network (no egress). */
  internal?: boolean;
}

/** Options accepted by {@link Client.listBridgeNetworks}. */
export interface ListBridgeNetworksOptions {
  /** Filter by labels — each entry becomes one `label=k=v` pair (AND). */
  labels?: Record<string, string>;
}

/** Options accepted by {@link Client.deleteBridgeNetwork}. */
export interface DeleteBridgeNetworkOptions {
  /**
   * When true, detach any remaining attachments before removal. Maps
   * to `?force=true`. Without it, a network with live attachments
   * returns 409 Conflict.
   */
  force?: boolean;
}

/** Body for {@link Client.connectBridgeNetwork}. */
export interface ConnectBridgeNetworkInput {
  /** Container id (runtime id, not the API's id string). */
  containerId: string;
  /** DNS aliases to advertise on this network. */
  aliases?: string[];
  /** Static IPv4 pin, validated server-side as `Ipv4Addr`. */
  ipv4Address?: string;
}

/** Options accepted by {@link Client.disconnectBridgeNetwork}. */
export interface DisconnectBridgeNetworkOptions {
  /** Force detach even if the runtime returns an error. */
  force?: boolean;
}

/** Ergonomic ZLayer SDK client. */
export class Client {
  /** Raw generated API classes. Prefer the named methods below. */
  public readonly raw: RawApis;

  private readonly fetchApi: FetchLike;
  private readonly basePath: string;
  private readonly token: string | undefined;

  constructor(options: ClientOptions = {}) {
    const transport = options.fetchApi
      ? { fetchApi: options.fetchApi, basePath: options.host ?? "http://unix" }
      : buildTransport({
          ...(options.socket !== undefined ? { socket: options.socket } : {}),
          ...(options.host !== undefined ? { host: options.host } : {}),
        });

    this.fetchApi = transport.fetchApi;
    this.basePath = transport.basePath;
    this.token = options.token;

    const configuration = new Configuration({
      basePath: this.basePath,
      fetchApi: this.fetchApi as unknown as Configuration["fetchApi"],
      ...(this.token ? { accessToken: this.token } : {}),
    });

    this.raw = {
      build: new BuildApi(configuration),
      containers: new ContainersApi(configuration),
      containerNetworks: new ContainerNetworksApi(configuration),
      credentials: new CredentialsApi(configuration),
      deployments: new DeploymentsApi(configuration),
      events: new EventsApi(configuration),
      images: new ImagesApi(configuration),
      services: new ServicesApi(configuration),
      volumes: new VolumesApi(configuration),
      configuration,
    };
  }

  // -------------------------------------------------------------------
  // Deployment lifecycle
  // -------------------------------------------------------------------

  /**
   * Create a new deployment from a spec YAML string.
   *
   * Python equivalent: `client.deploy(spec_yaml)`.
   * Generated target: `DeploymentsApi.createDeployment({createDeploymentRequest: {spec}})`.
   */
  async deploy(specYaml: string): Promise<DeploymentDetails> {
    return this.raw.deployments.createDeployment({
      createDeploymentRequest: { spec: specYaml },
    });
  }

  /**
   * Return the full details of a deployment by name.
   * Generated target: `DeploymentsApi.getDeployment({name})`.
   */
  async status(deployment: string): Promise<DeploymentDetails> {
    return this.raw.deployments.getDeployment({ name: deployment });
  }

  /**
   * Delete a deployment by name (removes all associated services, etc.).
   * Generated target: `DeploymentsApi.deleteDeployment({name})`.
   */
  async stop(deployment: string): Promise<void> {
    await this.raw.deployments.deleteDeployment({ name: deployment });
  }

  /** List all deployments. */
  async deployments(): Promise<DeploymentSummary[]> {
    return this.raw.deployments.listDeployments();
  }

  // -------------------------------------------------------------------
  // Container / process management
  // -------------------------------------------------------------------

  /**
   * List containers (`docker ps` equivalent).
   *
   * Generated target: `ContainersApi.listContainers({label?})`.
   */
  async ps(opts: { label?: string } = {}): Promise<ContainerInfo[]> {
    return this.raw.containers.listContainers({
      label: opts.label ?? null,
    });
  }

  /**
   * Fetch container logs.
   * Generated target: `ContainersApi.getContainerLogs({id, tail?, follow?})`.
   */
  async logs(containerId: string, opts: LogsOptions = {}): Promise<string> {
    const request: { id: string; tail?: number; follow?: boolean } = { id: containerId };
    if (opts.tail !== undefined) request.tail = opts.tail;
    if (opts.follow !== undefined) request.follow = opts.follow;
    return this.raw.containers.getContainerLogs(request);
  }

  /**
   * Kill a container with a specific signal (default `SIGKILL`).
   * Generated target: `ContainersApi.killContainer({id, killContainerRequest: {signal?}})`.
   */
  async kill(containerId: string, signal?: string): Promise<void> {
    await this.raw.containers.killContainer({
      id: containerId,
      killContainerRequest: signal ? { signal } : {},
    });
  }

  /**
   * Gracefully stop a running container (Docker-compat `POST /containers/{id}/stop`).
   *
   * The container is **not** removed — use {@link Client.stop} (deployment-level)
   * or the raw `ContainersApi.deleteContainer` call if you want stop-and-remove.
   * Idempotent: calling stop on an already-stopped container is not an error.
   *
   * Generated target: `ContainersApi.stopContainer({id, stopContainerRequest: {timeout?}})`.
   */
  async stopContainer(
    containerId: string,
    opts: ContainerStopOptions = {},
  ): Promise<void> {
    await this.raw.containers.stopContainer({
      id: containerId,
      stopContainerRequest:
        opts.timeoutSeconds !== undefined ? { timeout: opts.timeoutSeconds } : {},
    });
  }

  /**
   * Start a previously-created (stopped) container.
   *
   * Useful for re-starting a container that was stopped via
   * {@link Client.stopContainer} without being removed.
   *
   * Generated target: `ContainersApi.startContainer({id})`.
   */
  async startContainer(containerId: string): Promise<void> {
    await this.raw.containers.startContainer({ id: containerId });
  }

  /**
   * Restart a container (stop-then-start) with an optional graceful shutdown
   * timeout. Mirrors Docker-compat `POST /containers/{id}/restart`.
   *
   * Generated target: `ContainersApi.restartContainer({id, restartContainerRequest: {timeout?}})`.
   */
  async restartContainer(
    containerId: string,
    opts: ContainerStopOptions = {},
  ): Promise<void> {
    await this.raw.containers.restartContainer({
      id: containerId,
      restartContainerRequest:
        opts.timeoutSeconds !== undefined ? { timeout: opts.timeoutSeconds } : {},
    });
  }

  /**
   * Send a signal to a running container (Docker-compat
   * `POST /containers/{id}/kill`). When `opts.signal` is omitted the runtime
   * sends `SIGKILL`. Accepted signals: `SIGKILL`, `SIGTERM`, `SIGINT`,
   * `SIGHUP`, `SIGUSR1`, `SIGUSR2` (with or without the `SIG` prefix).
   *
   * Prefer this over the deployment-level {@link Client.kill} when you have a
   * container id (not a service name).
   *
   * Generated target: `ContainersApi.killContainer({id, killContainerRequest: {signal?}})`.
   */
  async killContainer(
    containerId: string,
    opts: ContainerKillOptions = {},
  ): Promise<void> {
    await this.raw.containers.killContainer({
      id: containerId,
      killContainerRequest: opts.signal ? { signal: opts.signal } : {},
    });
  }

  // -------------------------------------------------------------------
  // Container CRUD (container-level, not deployment-level)
  // -------------------------------------------------------------------

  /**
   * Create (and start) a container directly, bypassing the deployment
   * layer. Mirrors Docker-compat `POST /containers/create` + `start`.
   *
   * Pass-through fields include `image`, `command`, `env`, `labels`,
   * `resources`, `workDir`, `ports`, `networks`, `healthCheck`,
   * `restartPolicy`, `hostname`, `dns`, `extraHosts`, `volumes`,
   * `pullPolicy`, `registryCredentialId`, and inline `registryAuth`.
   *
   * Generated target:
   * `ContainersApi.createContainer({createContainerRequest: {...}})`.
   */
  async createContainer(req: CreateContainerInput): Promise<ContainerInfo> {
    return this.raw.containers.createContainer({
      createContainerRequest: req,
    });
  }

  /**
   * Fetch details of a single container by id.
   *
   * Generated target: `ContainersApi.getContainer({id})`.
   */
  async getContainer(id: string): Promise<ContainerInfo> {
    return this.raw.containers.getContainer({ id });
  }

  /**
   * List containers with richer filter semantics than {@link Client.ps}.
   *
   * When `opts.label` is set, each `k=v` pair is flattened into a
   * single `label=k=v` query arg (the generated client accepts one
   * `label` string; ANDed label filters are emulated by the
   * `?label=a=1&label=b=2` form the server expects — passing more than
   * one entry falls back to issuing separate list calls is not needed
   * because the server handler supports repeated `label` params, and
   * the underlying fetch URL-encodes each entry).
   *
   * Generated target: `ContainersApi.listContainers({label?})`.
   */
  async listContainers(opts: ListContainersOptions = {}): Promise<ContainerInfo[]> {
    if (!opts.label || Object.keys(opts.label).length === 0) {
      return this.raw.containers.listContainers({ label: null });
    }
    // The generated client types `label` as a single string. The daemon
    // accepts repeated `label` query params — we send one combined entry
    // (the handler splits on `=`). For multiple labels callers can use
    // the raw API (`raw.containers.listContainersRaw`) with its
    // `initOverrides` to pass additional query string entries.
    const entries = Object.entries(opts.label);
    const [firstKey, firstValue] = entries[0]!;
    return this.raw.containers.listContainers({
      label: `${firstKey}=${firstValue}`,
    });
  }

  /**
   * Remove a container. By default the daemon sends `SIGTERM` (with a
   * 30-second grace period) before unlinking; `opts.force` is accepted
   * for forward-compatibility when the server gains `?force=true`.
   *
   * Generated target: `ContainersApi.deleteContainer({id})`.
   */
  async deleteContainer(
    id: string,
    _opts: DeleteContainerOptions = {},
  ): Promise<void> {
    await this.raw.containers.deleteContainer({ id });
  }

  /**
   * Block until the container exits and return its exit classification
   * (exit code, optional signal name, optional reason, optional finish
   * timestamp).
   *
   * Generated target: `ContainersApi.waitContainer({id})`.
   */
  async waitContainer(id: string): Promise<ContainerWaitResponse> {
    return this.raw.containers.waitContainer({ id });
  }

  // -------------------------------------------------------------------
  // Streaming: exec, stats, events
  // -------------------------------------------------------------------

  /**
   * Execute a command in a running container and yield each line of
   * stdout/stderr as it appears, finishing with a single `exit` frame.
   *
   * Wire format: `POST /api/v1/containers/{id}/exec?stream=true` with a
   * Server-Sent Events response. The generated OpenAPI operation has no
   * first-class streaming variant, so this method drops down to the
   * shared {@link FetchLike} transport. Auth headers + Unix-socket
   * routing match the rest of the facade.
   */
  async *execStream(
    id: string,
    opts: ExecStreamOptions,
  ): AsyncIterable<ExecStreamEvent> {
    if (!opts.command || opts.command.length === 0) {
      throw new Error("execStream: command must be a non-empty argv array");
    }

    const url = `${this.basePath}/api/v1/containers/${encodeURIComponent(id)}/exec?stream=true`;
    const headers: Record<string, string> = {
      "Content-Type": "application/json",
      Accept: "text/event-stream",
    };
    if (this.token) headers["Authorization"] = `Bearer ${this.token}`;

    const body: Record<string, unknown> = { command: opts.command };
    if (opts.env !== undefined) body["env"] = opts.env;
    if (opts.tty !== undefined) body["tty"] = opts.tty;

    const response = await this.fetchApi(url, {
      method: "POST",
      headers,
      body: JSON.stringify(body),
    });
    if (!response.ok) {
      const detail = await response.text().catch(() => "");
      throw new Error(
        `execStream failed: HTTP ${response.status} ${response.statusText}${detail ? ` — ${detail}` : ""}`,
      );
    }

    for await (const frame of consumeSseStream(response)) {
      if (frame.event === "stdout") {
        yield { kind: "stdout", data: frame.data };
      } else if (frame.event === "stderr") {
        yield { kind: "stderr", data: frame.data };
      } else if (frame.event === "exit") {
        let exitCode = 0;
        try {
          const parsed = JSON.parse(frame.data) as { exit_code?: number };
          if (typeof parsed.exit_code === "number") exitCode = parsed.exit_code;
        } catch {
          // Fall back to parsing the raw payload as a bare integer.
          const n = Number.parseInt(frame.data, 10);
          if (Number.isFinite(n)) exitCode = n;
        }
        yield { kind: "exit", exitCode };
        return;
      }
      // Unknown event names are ignored — lets the daemon add new frame
      // kinds without breaking existing consumers.
    }
  }

  /**
   * Subscribe to a container's resource statistics. Yields one sample
   * per server-emitted frame until the stream closes.
   *
   * Wire format: `GET /api/v1/containers/{id}/stats?stream=true` with a
   * Server-Sent Events response carrying a JSON-encoded
   * {@link ContainerStatsResponse} as each frame's `data:` payload.
   */
  async *streamStats(
    id: string,
    opts: StatsStreamOptions = {},
  ): AsyncIterable<ContainerStatsResponse> {
    const params = new URLSearchParams({ stream: "true" });
    if (opts.intervalSeconds !== undefined) {
      params.set("interval", String(opts.intervalSeconds));
    }
    const url = `${this.basePath}/api/v1/containers/${encodeURIComponent(id)}/stats?${params.toString()}`;
    const headers: Record<string, string> = { Accept: "text/event-stream" };
    if (this.token) headers["Authorization"] = `Bearer ${this.token}`;

    const response = await this.fetchApi(url, { method: "GET", headers });
    if (!response.ok) {
      const detail = await response.text().catch(() => "");
      throw new Error(
        `streamStats failed: HTTP ${response.status} ${response.statusText}${detail ? ` — ${detail}` : ""}`,
      );
    }

    for await (const frame of consumeSseStream(response)) {
      if (!frame.data) continue;
      const parsed = JSON.parse(frame.data);
      yield ContainerStatsResponseFromJSON(parsed);
    }
  }

  /**
   * Subscribe to the cluster-wide container lifecycle event stream.
   *
   * Wire format: `GET /api/v1/events?follow=true[&label=k=v]*`. Each
   * SSE frame's `data:` payload is a JSON-encoded
   * {@link ContainerEvent}. Pass `opts.labels` to restrict the feed to
   * containers carrying specific labels (AND semantics).
   */
  async *streamEvents(
    opts: EventsStreamOptions = {},
  ): AsyncIterable<ContainerEvent> {
    const params = new URLSearchParams({ follow: "true" });
    if (opts.labels) {
      for (const [k, v] of Object.entries(opts.labels)) {
        params.append("label", `${k}=${v}`);
      }
    }
    const url = `${this.basePath}/api/v1/events?${params.toString()}`;
    const headers: Record<string, string> = { Accept: "text/event-stream" };
    if (this.token) headers["Authorization"] = `Bearer ${this.token}`;

    const response = await this.fetchApi(url, { method: "GET", headers });
    if (!response.ok) {
      const detail = await response.text().catch(() => "");
      throw new Error(
        `streamEvents failed: HTTP ${response.status} ${response.statusText}${detail ? ` — ${detail}` : ""}`,
      );
    }

    for await (const frame of consumeSseStream(response)) {
      if (!frame.data) continue;
      const parsed = JSON.parse(frame.data);
      yield ContainerEventFromJSON(parsed);
    }
  }

  // -------------------------------------------------------------------
  // Service exec / scale
  // -------------------------------------------------------------------

  /**
   * Execute a command inside a service's container.
   *
   * This hits the daemon's service-scoped exec endpoint
   * (`POST /api/v1/deployments/{deployment}/services/{service}/exec`),
   * which matches the Python `Client.exec` signature. The endpoint is not
   * currently documented in OpenAPI (see `scripts/generate-clients.sh`),
   * so we call it directly through the same transport the generated
   * client uses — that way Unix-socket routing + auth headers stay
   * consistent.
   *
   * Returns the daemon's JSON response verbatim (stdout, stderr, exit
   * code). The shape is defined by the `exec_command` handler in
   * `zlayer-api`.
   */
  async exec(
    deployment: string,
    service: string,
    cmd: string[],
  ): Promise<ExecResult> {
    const url = `${this.basePath}/api/v1/deployments/${encodeURIComponent(deployment)}/services/${encodeURIComponent(service)}/exec`;
    const headers: Record<string, string> = {
      "Content-Type": "application/json",
      Accept: "application/json",
    };
    if (this.token) {
      headers["Authorization"] = `Bearer ${this.token}`;
    }
    const response = await this.fetchApi(url, {
      method: "POST",
      headers,
      body: JSON.stringify({ command: cmd }),
    });
    if (!response.ok) {
      const body = await response.text().catch(() => "");
      throw new Error(
        `exec ${deployment}/${service} failed: HTTP ${response.status} ${response.statusText}${body ? ` — ${body}` : ""}`,
      );
    }
    return (await response.json()) as ExecResult;
  }

  /**
   * Scale a service to a specific replica count.
   * Generated target: `ServicesApi.scaleService({deployment, service, scaleRequest: {replicas}})`.
   */
  async scale(
    deployment: string,
    service: string,
    replicas: number,
  ): Promise<ServiceDetails> {
    return this.raw.services.scaleService({
      deployment,
      service,
      scaleRequest: { replicas },
    });
  }

  /** List services in a deployment. */
  async services(deployment: string): Promise<ServiceSummary[]> {
    return this.raw.services.listServices({ deployment });
  }

  // -------------------------------------------------------------------
  // Build / image ops
  // -------------------------------------------------------------------

  /**
   * Trigger a server-side build against a context path visible to the daemon.
   * Generated target: `BuildApi.startBuildJson({buildRequestWithContext: {...}})`.
   *
   * Note: this is the JSON variant; it does not stream a tar context from
   * the client. For full local builds (streaming a tar of the local
   * directory), callers should shell out to `zlayer build` for now.
   */
  async build(
    contextPath: string,
    tags: string[],
    opts: BuildOptions = {},
  ): Promise<TriggerBuildResponse> {
    return this.raw.build.startBuildJson({
      buildRequestWithContext: {
        contextPath,
        tags,
        ...(opts.buildArgs ? { buildArgs: opts.buildArgs } : {}),
        ...(opts.noCache !== undefined ? { noCache: opts.noCache } : {}),
        ...(opts.push !== undefined ? { push: opts.push } : {}),
        ...(opts.runtime !== undefined ? { runtime: opts.runtime } : {}),
        ...(opts.target !== undefined ? { target: opts.target } : {}),
      },
    });
  }

  /** Poll a build's status by id. */
  async buildStatus(id: string): Promise<BuildStatus> {
    return this.raw.build.getBuildStatus({ id });
  }

  /** Return the available runtime templates (`node`, `python`, etc.). */
  async runtimes(): Promise<TemplateInfo[]> {
    return this.raw.build.listRuntimeTemplates();
  }

  /**
   * Create a new tag pointing at an already-cached image.
   * Generated target: `ImagesApi.tagImageHandler({tagImageRequest: {source, target}})`.
   */
  async tag(source: string, target: string): Promise<void> {
    await this.raw.images.tagImageHandler({
      tagImageRequest: { source, target },
    });
  }

  /**
   * Pull an image from a remote registry by reference
   * (e.g. `docker.io/library/nginx:latest`).
   * Generated target: `ImagesApi.pullImageHandler({pullImageRequest: {reference}})`.
   */
  async pull(reference: string): Promise<PullImageResponse> {
    return this.raw.images.pullImageHandler({
      pullImageRequest: { reference },
    });
  }

  /**
   * Pull an OCI image into the runtime's local cache, with optional
   * pull-policy override. Blocking: resolves after the image is stored
   * locally (or the pull fails).
   *
   * The higher-level {@link Client.pull} method takes just a reference; this
   * variant exposes the full `PullImageRequest` body so callers can override
   * the pull policy (`"always"` | `"if_not_present"` | `"never"`).
   *
   * Generated target: `ImagesApi.pullImageHandler({pullImageRequest: {reference, pullPolicy?}})`.
   */
  async pullImage(req: PullImageInput): Promise<PullImageResponse> {
    return this.raw.images.pullImageHandler({
      pullImageRequest: {
        reference: req.reference,
        ...(req.pullPolicy !== undefined ? { pullPolicy: req.pullPolicy } : {}),
      },
    });
  }

  // -------------------------------------------------------------------
  // Registry credentials
  // -------------------------------------------------------------------

  /**
   * List all registry credentials (metadata only — passwords are never
   * returned). Admin-only on the server side.
   *
   * Generated target: `CredentialsApi.listRegistryCredentials()`.
   */
  async listRegistryCredentials(): Promise<RegistryCredentialResponse[]> {
    return this.raw.credentials.listRegistryCredentials();
  }

  /**
   * Create a new registry credential. The password is stored encrypted and
   * never round-tripped back to clients. Admin-only on the server side.
   *
   * Generated target:
   * `CredentialsApi.createRegistryCredential({createRegistryCredentialRequest: {...}})`.
   */
  async createRegistryCredential(
    req: CreateRegistryCredentialInput,
  ): Promise<RegistryCredentialResponse> {
    return this.raw.credentials.createRegistryCredential({
      createRegistryCredentialRequest: {
        registry: req.registry,
        username: req.username,
        password: req.password,
        authType: req.authType ?? "basic",
      },
    });
  }

  /**
   * Delete a registry credential by id. Admin-only on the server side.
   *
   * Generated target: `CredentialsApi.deleteRegistryCredential({id})`.
   */
  async deleteRegistryCredential(id: string): Promise<void> {
    await this.raw.credentials.deleteRegistryCredential({ id });
  }

  // -------------------------------------------------------------------
  // Volumes
  // -------------------------------------------------------------------

  /**
   * Create a named volume. The daemon writes a `.metadata.json` sidecar
   * capturing labels, size, tier, and creation timestamp.
   *
   * Generated target: `VolumesApi.createVolume({createVolumeRequest: {...}})`.
   */
  async createVolume(req: CreateVolumeInput): Promise<VolumeInfo> {
    return this.raw.volumes.createVolume({
      createVolumeRequest: {
        name: req.name,
        ...(req.size !== undefined ? { size: req.size } : {}),
        ...(req.tier !== undefined ? { tier: req.tier } : {}),
        ...(req.labels !== undefined ? { labels: req.labels } : {}),
      },
    });
  }

  /** List all named volumes. Generated target: `VolumesApi.listVolumes()`. */
  async listVolumes(): Promise<VolumeInfo[]> {
    return this.raw.volumes.listVolumes();
  }

  /**
   * Inspect a single volume by name. Generated target:
   * `VolumesApi.getVolume({name})`.
   */
  async getVolume(name: string): Promise<VolumeInfo> {
    return this.raw.volumes.getVolume({ name });
  }

  /**
   * Remove a named volume. By default refuses when the volume is
   * non-empty or still mounted by a container — pass `force: true` to
   * override both checks.
   *
   * Generated target: `VolumesApi.deleteVolume({name, force})`.
   */
  async deleteVolume(
    name: string,
    opts: DeleteVolumeOptions = {},
  ): Promise<void> {
    await this.raw.volumes.deleteVolume({ name, force: opts.force ?? false });
  }

  // -------------------------------------------------------------------
  // Bridge / overlay networks
  // -------------------------------------------------------------------

  /**
   * Create a user-defined bridge or overlay network.
   *
   * Generated target:
   * `ContainerNetworksApi.createContainerNetwork({createBridgeNetworkRequest: {...}})`.
   */
  async createBridgeNetwork(
    req: CreateBridgeNetworkInput,
  ): Promise<BridgeNetwork> {
    return this.raw.containerNetworks.createContainerNetwork({
      createBridgeNetworkRequest: {
        name: req.name,
        ...(req.driver !== undefined ? { driver: req.driver } : {}),
        ...(req.subnet !== undefined ? { subnet: req.subnet } : {}),
        ...(req.labels !== undefined ? { labels: req.labels } : {}),
        ...(req.internal !== undefined ? { internal: req.internal } : {}),
      },
    });
  }

  /**
   * List bridge networks, optionally filtered by label.
   *
   * Multi-label filters are flattened to a single `label=k=v` entry
   * because the generated client accepts only one string — callers
   * needing true AND filtering across several labels can fall back to
   * `raw.containerNetworks.listContainerNetworksRaw` and pass a custom
   * query string via `initOverrides`.
   *
   * Generated target:
   * `ContainerNetworksApi.listContainerNetworks({label?})`.
   */
  async listBridgeNetworks(
    opts: ListBridgeNetworksOptions = {},
  ): Promise<BridgeNetwork[]> {
    if (!opts.labels || Object.keys(opts.labels).length === 0) {
      return this.raw.containerNetworks.listContainerNetworks({ label: null });
    }
    const entries = Object.entries(opts.labels);
    const [firstKey, firstValue] = entries[0]!;
    return this.raw.containerNetworks.listContainerNetworks({
      label: `${firstKey}=${firstValue}`,
    });
  }

  /**
   * Inspect a bridge network by id or name.
   *
   * Generated target:
   * `ContainerNetworksApi.getContainerNetwork({idOrName})`.
   */
  async getBridgeNetwork(idOrName: string): Promise<BridgeNetworkDetails> {
    return this.raw.containerNetworks.getContainerNetwork({ idOrName });
  }

  /**
   * Delete a bridge network. Without `force`, the daemon refuses if any
   * container is still attached.
   *
   * Generated target:
   * `ContainerNetworksApi.deleteContainerNetwork({idOrName, force?})`.
   */
  async deleteBridgeNetwork(
    idOrName: string,
    opts: DeleteBridgeNetworkOptions = {},
  ): Promise<void> {
    await this.raw.containerNetworks.deleteContainerNetwork({
      idOrName,
      ...(opts.force !== undefined ? { force: opts.force } : {}),
    });
  }

  /**
   * Attach a container to a bridge network.
   *
   * Generated target:
   * `ContainerNetworksApi.connectContainerNetwork({idOrName, connectBridgeNetworkRequest: {...}})`.
   */
  async connectBridgeNetwork(
    idOrName: string,
    attach: ConnectBridgeNetworkInput,
  ): Promise<void> {
    await this.raw.containerNetworks.connectContainerNetwork({
      idOrName,
      connectBridgeNetworkRequest: {
        containerId: attach.containerId,
        ...(attach.aliases !== undefined ? { aliases: attach.aliases } : {}),
        ...(attach.ipv4Address !== undefined
          ? { ipv4Address: attach.ipv4Address }
          : {}),
      },
    });
  }

  /**
   * Detach a container from a bridge network.
   *
   * Generated target:
   * `ContainerNetworksApi.disconnectContainerNetwork({idOrName, disconnectBridgeNetworkRequest: {...}})`.
   */
  async disconnectBridgeNetwork(
    idOrName: string,
    containerId: string,
    opts: DisconnectBridgeNetworkOptions = {},
  ): Promise<void> {
    await this.raw.containerNetworks.disconnectContainerNetwork({
      idOrName,
      disconnectBridgeNetworkRequest: {
        containerId,
        ...(opts.force !== undefined ? { force: opts.force } : {}),
      },
    });
  }
}

/** Response shape from `POST /api/v1/deployments/{d}/services/{s}/exec`. */
export interface ExecResult {
  /** Captured stdout. */
  stdout?: string;
  /** Captured stderr. */
  stderr?: string;
  /** Exit code from the remote process (nullable if the process was killed). */
  exitCode?: number | null;
  /** Extra free-form fields the daemon may return. */
  [key: string]: unknown;
}
