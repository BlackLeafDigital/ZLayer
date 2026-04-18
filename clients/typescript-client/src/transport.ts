/**
 * Transport helpers for the ZLayer TypeScript client.
 *
 * The ZLayer daemon listens on one of:
 *   * A Unix domain socket (default on Linux/macOS, e.g. `/var/run/zlayer.sock`)
 *   * A TCP/HTTP endpoint (e.g. `tcp://127.0.0.1:3669` on Windows, or when
 *     the user explicitly binds an HTTP listener via `zlayer serve --bind`).
 *
 * We use {@link https://undici.nodejs.org undici}'s `Agent` to attach a
 * Unix-socket `connect` hook to the global `fetch` API — the
 * auto-generated OpenAPI client accepts a custom `fetchApi`, so this
 * transports the request over AF_UNIX transparently.
 */
import { Agent, fetch as undiciFetch } from "undici";

/**
 * The generic fetch signature expected by the generated runtime.
 *
 * We don't use the DOM-specific `RequestInfo` type here because this
 * package targets pure Node and avoids the DOM lib. The generated
 * OpenAPI client only ever calls fetch with a URL string, so keeping
 * `input: string | URL` is sufficient in practice.
 */
export type FetchLike = (
  input: string | URL,
  init?: RequestInit,
) => Promise<Response>;

/** Options for building a fetch bound to a particular daemon transport. */
export interface TransportOptions {
  /**
   * Path to a Unix domain socket (e.g. `/var/run/zlayer.sock`). Mutually
   * exclusive with `host`. When set, requests are dispatched via an undici
   * Agent whose `connect` hook opens AF_UNIX.
   */
  socket?: string;

  /**
   * HTTP base URL for the daemon (e.g. `http://127.0.0.1:3669`). Mutually
   * exclusive with `socket`. When set, the standard global fetch is used
   * and `basePath` is set to this host.
   */
  host?: string;
}

/**
 * Build a fetch function + OpenAPI basePath for the given transport.
 *
 * For Unix-socket transport, the returned `basePath` is a synthetic
 * `http://unix` — the hostname is irrelevant because undici's `Agent` with
 * a `connect.socketPath` will route regardless. The generated client will
 * concatenate `basePath + context.path`, so we return `http://unix` (no
 * trailing slash) and the path comes in already starting with `/api/v1/...`.
 */
export function buildTransport(opts: TransportOptions): {
  fetchApi: FetchLike;
  basePath: string;
} {
  if (opts.socket && opts.host) {
    throw new Error(
      "Pass either { socket } or { host }, not both. The daemon listens on one transport at a time.",
    );
  }

  if (opts.host) {
    const basePath = opts.host.replace(/\/+$/, "");
    // Use undici's fetch for consistency — it's the same interface as the
    // global fetch but gives us access to dispatchers / agents if callers
    // ever want to override credentials.
    const fetchApi: FetchLike = ((input, init) =>
      undiciFetch(input as any, init as any) as unknown as Promise<Response>);
    return { fetchApi, basePath };
  }

  // Default: Unix socket. Use the documented socket path if not given.
  const socketPath = opts.socket ?? defaultSocketPath();

  // `tcp://` is what ZLayer returns on Windows to signal "not a socket".
  // In that case we strip the scheme and use it as an HTTP host.
  if (socketPath.startsWith("tcp://")) {
    const basePath = "http://" + socketPath.slice("tcp://".length);
    const fetchApi: FetchLike = ((input, init) =>
      undiciFetch(input as any, init as any) as unknown as Promise<Response>);
    return { fetchApi, basePath };
  }

  const agent = new Agent({
    connect: {
      // `socketPath` on the connect options makes undici dial AF_UNIX.
      socketPath,
    },
  });

  const fetchApi: FetchLike = ((input, init) => {
    // We must pass the dispatcher via RequestInit. The generated client
    // doesn't know about dispatcher, but undici's fetch accepts it as a
    // non-standard extension of RequestInit.
    const merged = { ...(init ?? {}), dispatcher: agent } as RequestInit & {
      dispatcher: Agent;
    };
    return undiciFetch(input as any, merged as any) as unknown as Promise<Response>;
  });

  return { fetchApi, basePath: "http://unix" };
}

/**
 * Platform-aware default socket path. Matches the defaults embedded in the
 * ZLayer daemon (`zlayer_paths::ZLayerDirs::default_socket_path()`).
 */
export function defaultSocketPath(): string {
  if (process.platform === "win32") {
    return "tcp://127.0.0.1:3669";
  }
  if (process.platform === "darwin") {
    const home = process.env.HOME ?? ".";
    return `${home}/Library/Application Support/zlayer/run/zlayer.sock`;
  }
  return "/var/run/zlayer.sock";
}
