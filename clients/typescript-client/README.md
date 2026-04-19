# @zlayer/client

Ergonomic TypeScript SDK for the [ZLayer](https://zlayer.dev) container
orchestration daemon. Wraps the auto-generated OpenAPI client
(`@zlayer/api-client`) with a Unix-socket-aware fetch and a small surface of
convenience methods that mirror the [Python SDK](../../crates/zlayer-py).

## Install

```sh
npm install @zlayer/client
```

The package depends on `@zlayer/api-client` (the raw generated client) and on
[`undici`](https://undici.nodejs.org) for the AF_UNIX fetch transport.

## Quickstart

```ts
import { Client, ensureDaemon } from "@zlayer/client";

// Download the zlayer binary to ~/.local/share/zlayer/bin/ (userland).
await ensureDaemon();

// Defaults to the platform Unix socket:
//   * Linux:   /var/run/zlayer.sock
//   * macOS:   ~/Library/Application Support/zlayer/run/zlayer.sock
//   * Windows: tcp://127.0.0.1:3669 (HTTP)
const client = new Client();

console.log(await client.ps());
```

## Constructor

```ts
new Client({ socket?: string; host?: string; token?: string })
```

Pass exactly one of `socket` (Unix domain socket) or `host`
(HTTP base URL, e.g. `http://127.0.0.1:3669`). Omit both to use the
platform default (see above).

## Methods

| Method | Verb | Generated target |
| ------ | ---- | ---------------- |
| `deploy(specYaml)` | create deployment | `DeploymentsApi.createDeployment` |
| `ps({label?})` | list containers | `ContainersApi.listContainers` |
| `logs(containerId, {tail?, follow?})` | container logs | `ContainersApi.getContainerLogs` |
| `exec(deployment, service, cmd[])` | service exec | `POST /deployments/{d}/services/{s}/exec` |
| `build(contextPath, tags, opts?)` | start build | `BuildApi.startBuildJson` |
| `buildStatus(id)` | poll build | `BuildApi.getBuildStatus` |
| `runtimes()` | list runtime templates | `BuildApi.listRuntimeTemplates` |
| `stop(deployment)` | delete deployment | `DeploymentsApi.deleteDeployment` |
| `status(deployment)` | deployment details | `DeploymentsApi.getDeployment` |
| `scale(d, s, replicas)` | scale service | `ServicesApi.scaleService` |
| `kill(containerId, signal?)` | kill container | `ContainersApi.killContainer` |
| `tag(source, target)` | tag image | `ImagesApi.tagImageHandler` |
| `pull(reference)` | pull image | `ImagesApi.pullImageHandler` |

Full type definitions ship in `dist/index.d.ts`. For fine-grained access to
the generated request/response shapes, use the `client.raw` escape hatch:

```ts
const resp = await client.raw.deployments.listDeployments();
```

## Binary bootstrap (`ensureDaemon`)

`ensureDaemon()` downloads the release tarball from GitHub and extracts the
`zlayer` binary to `~/.local/share/zlayer/bin/zlayer`. The URL scheme matches
`install.py` at the repo root.

```ts
await ensureDaemon();                          // latest release, userland
await ensureDaemon({ version: "0.10.104" });   // pinned version
await ensureDaemon({ system: true });          // runs `zlayer daemon install` (prompts for sudo)
await ensureDaemon({ force: true });           // redownload even if a match exists
```

System mode (`system: true`) first bootstraps the userland binary, then
invokes `zlayer daemon install` with live terminal I/O so the CLI's own
sudo prompt reaches the user.

## Postinstall opt-in

The `postinstall` script in `package.json` is a **no-op by default**. To have
`npm install` also download the zlayer binary, set `ZLAYER_AUTO_INSTALL=1`:

```sh
ZLAYER_AUTO_INSTALL=1 npm install @zlayer/client
```

Any failure during an opt-in auto-install is logged but never fails the
install itself.

## Supported platforms

- Linux x86_64 / aarch64
- macOS x86_64 / aarch64
- Windows users can drive a daemon over HTTP (`new Client({ host: "http://localhost:3669" })`), but `ensureDaemon()` itself does not support Windows — install manually via WSL2 or the release tarball.

## License

Apache-2.0
