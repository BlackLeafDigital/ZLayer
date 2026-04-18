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
  ContainersApi,
  DeploymentsApi,
  ImagesApi,
  ServicesApi,
  type BuildStatus,
  type ContainerInfo,
  type DeploymentDetails,
  type DeploymentSummary,
  type PullImageResponse,
  type ServiceDetails,
  type ServiceSummary,
  type TemplateInfo,
  type TriggerBuildResponse,
} from "@zlayer/api-client";

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
  deployments: DeploymentsApi;
  images: ImagesApi;
  services: ServicesApi;
  configuration: Configuration;
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
      deployments: new DeploymentsApi(configuration),
      images: new ImagesApi(configuration),
      services: new ServicesApi(configuration),
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
