# WASM Portability

ZLayer's WASM builder emits BOTH a raw `.wasm` file and an OCI artifact on every build (unless opted out via `wasm.oci: false`). Your WASM code can run anywhere wasmtime / jco / wazero / runwasi / Spin / wasmCloud runs — the only question is which output to hand each consumer.

## Build outputs

- **Raw `.wasm`** — located at `<output-dir>/<module>.wasm`. Use this for standalone wasmtime, jco, wazero, local dev, and CI.
- **OCI artifact** — located at `<output-dir>/<module>-oci/` (`oci-layout`, `blobs/`, `index.json`). Use this for registry push, runwasi, Spin, and wasmCloud.

## OCI media types

| Slot | Value |
|---|---|
| Manifest | `application/vnd.oci.image.manifest.v1+json` |
| `artifactType` | `application/vnd.wasm.{component,module}.v1+wasm` |
| Config | `application/vnd.wasm.config.v0+json` (empty `{}`) |
| Layer | `application/wasm` |

These match the CNCF WASM-OCI WG spec and Bytecode Alliance conventions, so any spec-compliant loader will accept ZLayer's artifacts.

ZLayer's runtime detection also keys off these values in order: OCI 1.1+ `artifactType` first (most authoritative), then config `mediaType`, then layer `mediaType` as a final fallback. Artifacts produced by other spec-compliant builders (e.g. `wkg oci wit`, `wash push`) are therefore accepted by ZLayer as well — portability runs both directions.

## Consumer compatibility

| Consumer | Format | Status |
|---|---|---|
| `wkg` (Bytecode Alliance) | OCI | works |
| Spin (`spin registry pull`) | OCI | works |
| wasmCloud (`wash`) | OCI | works |
| runwasi / containerd-shim-wasm | OCI | works |
| Docker Desktop WASM | OCI (via runwasi) | works |
| ORAS / crane | OCI (generic) | works |
| wasmtime CLI | raw `.wasm` | use the raw file |
| jco | raw `.wasm` | use the raw file |
| wazero | raw `.wasm` | use the raw file |

## WIT worlds and portability

Portability is determined by the component's WIT world, **not** by the OCI envelope.

- **Portable worlds**: `wasi:cli/command@0.2.0`, `wasi:http/proxy@0.2.0`, and raw WASIp1 modules. These carry no ZLayer-specific imports and run on any wasmtime-based host.
- **ZLayer-only worlds** (see `wit/zlayer/world.wit`): `zlayer-plugin`, `zlayer-http-handler`, `zlayer-transformer`, `zlayer-authenticator`, `zlayer-rate-limiter`, `zlayer-middleware`, `zlayer-router`. These import `zlayer:plugin/*@0.1.0` (config, keyvalue, logging, secrets, metrics). External hosts will refuse to instantiate them because of unresolved imports.

If you want a portable artifact, target `wasi:cli/command` or `wasi:http/proxy` in your ZImagefile / cargo-component manifest. If you want to integrate deeply with ZLayer (config, secrets, kv, metrics), target one of our plugin worlds — and accept that the resulting component is ZLayer-bound.

## Opt out of OCI packaging

ZImagefile:

```yaml
wasm:
  target: component
  language: rust
  oci: false        # only emit raw .wasm
```

Or use `zlayer wasm build <path>` directly — that subcommand is compile-only and does no OCI packaging. Pair it with `zlayer wasm push <file> <ref>` when you want to publish to a registry on your own schedule rather than as part of every build.

## External consumer recipes

### wasmtime CLI (raw)

```bash
zlayer build -f ZImagefile.wasm .
wasmtime run ./out/myapp.wasm
```

### ORAS / crane (generic OCI pull)

```bash
zlayer build -f ZImagefile.wasm . -t ghcr.io/me/myapp:wasm
oras pull ghcr.io/me/myapp:wasm
```

### Spin

```bash
spin registry pull ghcr.io/me/myapp:wasm
spin up
```

### wasmCloud

```bash
wash call ghcr.io/me/myapp:wasm HttpServer.HandleRequest ...
```

### runwasi (containerd)

Point the WASM shim at the OCI reference; no extra conversion needed. Any containerd installation with `containerd-shim-wasmtime-v1` (or the equivalent wasmedge / wasmer shim) resolves the artifact via the standard OCI fetch path.

## Which output do I hand out?

- Publishing to a public registry for anyone's consumption → **OCI**. It's the interchange format every loader understands.
- Running locally or in CI with just a wasmtime/jco/wazero binary → **raw `.wasm`**. Skip the registry entirely.
- Using ZLayer's plugin worlds → either format works inside ZLayer, but the component is not portable regardless of envelope; pick based on how you distribute it.

## Troubleshooting

- **"unknown import" / "unresolved import zlayer:plugin/..."** — The component targets a ZLayer plugin world. Rebuild against `wasi:cli/command` or `wasi:http/proxy` for external hosts, or run it inside ZLayer.
- **Registry rejects the push** — Some older registries do not accept OCI 1.1 artifacts with a non-image `artifactType`. Use a registry that supports OCI 1.1 (ghcr.io, Docker Hub, Zot, Harbor 2.8+). ORAS can also be used to force-push.
- **`wasmtime run` fails on the OCI artifact** — wasmtime CLI does not speak OCI; point it at the raw `.wasm` file instead.
