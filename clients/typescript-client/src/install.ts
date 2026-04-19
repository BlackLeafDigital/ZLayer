/**
 * Bootstrap the ``zlayer`` binary from Node.js.
 *
 * Mirrors the Python `zlayer.ensure_daemon` helper — downloads the
 * release tarball from GitHub to a userland directory by default, or
 * escalates to ``zlayer daemon install`` for system-wide placement.
 *
 * The URL scheme must stay in sync with ``install.py`` and
 * ``crates/zlayer-py/python/zlayer/_install.py``:
 *
 *   https://github.com/BlackLeafDigital/ZLayer/releases/download/v{version}/zlayer-{version}-{os}-{arch}.tar.gz
 */
import { execFile as execFileCb, spawn } from "node:child_process";
import { promises as fs, createWriteStream, existsSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import { pipeline } from "node:stream/promises";
import { promisify } from "node:util";
import { fetch as undiciFetch } from "undici";
import { createHash } from "node:crypto";

const execFile = promisify(execFileCb);

// ---------------------------------------------------------------------------
// Constants (kept in sync with install.py)
// ---------------------------------------------------------------------------

const GITHUB_REPO = "BlackLeafDigital/ZLayer";
const GITHUB_API_LATEST = `https://api.github.com/repos/${GITHUB_REPO}/releases/latest`;
const GITHUB_API_TAG = (tag: string) =>
  `https://api.github.com/repos/${GITHUB_REPO}/releases/tags/${tag}`;
const GITHUB_DOWNLOAD = (tag: string, filename: string) =>
  `https://github.com/${GITHUB_REPO}/releases/download/${tag}/${filename}`;

const BINARY_NAME = process.platform === "win32" ? "zlayer.exe" : "zlayer";
const USER_AGENT = "zlayer-node-install/1.0";

/**
 * Default userland binary directory. Matches the Python installer:
 *   ~/.local/share/zlayer/bin/zlayer
 */
export function defaultBinaryDir(): string {
  return join(homedir(), ".local", "share", "zlayer", "bin");
}

/** Error raised by {@link ensureDaemon} for any bootstrapping failure. */
export class ZLayerInstallError extends Error {
  public override readonly cause?: unknown;

  constructor(message: string, cause?: unknown) {
    super(message);
    this.name = "ZLayerInstallError";
    if (cause !== undefined) {
      this.cause = cause;
    }
  }
}

/** Options accepted by {@link ensureDaemon}. */
export interface EnsureDaemonOptions {
  /**
   * If true, run ``zlayer daemon install`` after downloading to escalate
   * to a system-wide install. The binary prompts for sudo itself.
   */
  system?: boolean;
  /**
   * Specific release version to install (e.g. ``"0.10.104"`` or
   * ``"v0.10.104"``). When omitted, the latest GitHub release is used.
   */
  version?: string;
  /** Re-download even if a matching binary already exists. */
  force?: boolean;
  /**
   * Override the install directory. Defaults to {@link defaultBinaryDir}.
   * The binary is written atomically as `zlayer.new` then renamed.
   */
  dir?: string;
}

/**
 * Ensure a `zlayer` binary is available. Returns the absolute path to it.
 *
 * Behavior in the two modes:
 *
 *   * `system: false` (default): download the release tarball from GitHub,
 *     extract the binary, and place it under `~/.local/share/zlayer/bin/`.
 *     Path is returned. No sudo involved.
 *   * `system: true`: bootstrap the userland binary first, then invoke
 *     `zlayer daemon install`. That subcommand handles its own privilege
 *     prompt (typically via sudo). The function does *not* wrap the
 *     invocation in sudo itself — the user's credentials stay under the
 *     CLI's control.
 */
export async function ensureDaemon(opts: EnsureDaemonOptions = {}): Promise<string> {
  if (opts.system) {
    const binary = await ensureDaemon({ ...opts, system: false });
    await runDaemonInstall(binary);
    return binary;
  }

  const dir = opts.dir ?? defaultBinaryDir();
  await fs.mkdir(dir, { recursive: true });
  const dest = join(dir, BINARY_NAME);

  // Short-circuit if existing binary already matches the requested version.
  if (!opts.force && existsSync(dest)) {
    try {
      const current = await runBinaryVersion(dest);
      if (current) {
        if (!opts.version) return dest;
        if (versionMatches(current, opts.version)) return dest;
      }
    } catch {
      // Fall through to redownload.
    }
  }

  const release = await fetchRelease(opts.version);
  const tagName = release.tag_name ?? "";
  if (!tagName) {
    throw new ZLayerInstallError(
      "GitHub release response missing 'tag_name'; cannot proceed.",
    );
  }
  const releaseVersion = tagName.startsWith("v") ? tagName.slice(1) : tagName;
  const assetUrl = resolveAssetUrl(release, releaseVersion, tagName);

  const work = await fs.mkdtemp(join(tmpdir(), "zlayer-node-install-"));
  try {
    const archivePath = join(work, assetUrl.split("/").pop() ?? "zlayer.tar.gz");
    const sha = await downloadToFile(assetUrl, archivePath);

    const stat = await fs.stat(archivePath);
    if (stat.size < 1024) {
      throw new ZLayerInstallError(
        `Downloaded archive is implausibly small (${stat.size} bytes); aborting.`,
      );
    }
    // No published SHA256SUMS yet — log the digest so the caller can
    // cross-check against the GitHub release page manually.
    console.error(
      `[zlayer] downloaded ${archivePath.split("/").pop()} ` +
        `(${stat.size} bytes, sha256=${sha})`,
    );

    const extractDir = join(work, "extract");
    await fs.mkdir(extractDir, { recursive: true });
    const extracted = await extractBinary(archivePath, extractDir);

    const staging = dest + ".new";
    await fs.copyFile(extracted, staging);
    await fs.chmod(staging, 0o755);
    await fs.rename(staging, dest);
  } finally {
    await fs.rm(work, { recursive: true, force: true }).catch(() => undefined);
  }

  // Sanity check — newly installed binary should run.
  const final = await runBinaryVersion(dest);
  if (!final) {
    throw new ZLayerInstallError(
      `Installed ${dest} but '${dest} --version' failed; binary may be corrupt or incompatible.`,
    );
  }
  return dest;
}

// ---------------------------------------------------------------------------
// Platform detection (mirrors install.py::Platform)
// ---------------------------------------------------------------------------

function detectOs(): "linux" | "darwin" | "windows" {
  switch (process.platform) {
    case "linux":
      return "linux";
    case "darwin":
      return "darwin";
    case "win32":
      return "windows";
    default:
      throw new ZLayerInstallError(
        `Unsupported operating system: ${process.platform}. ` +
          "Node's ensureDaemon() supports Linux and macOS; Windows users should use WSL2.",
      );
  }
}

function detectArch(): "amd64" | "arm64" {
  switch (process.arch) {
    case "x64":
      return "amd64";
    case "arm64":
      return "arm64";
    default:
      throw new ZLayerInstallError(`Unsupported architecture: ${process.arch}`);
  }
}

function artifactSuffix(): string {
  const os = detectOs();
  if (os === "windows") {
    throw new ZLayerInstallError(
      "ensureDaemon() does not support Windows directly. " +
        "Use WSL2 or install manually from the GitHub releases page.",
    );
  }
  return `${os}-${detectArch()}`;
}

// ---------------------------------------------------------------------------
// GitHub release metadata
// ---------------------------------------------------------------------------

interface ReleaseAsset {
  name?: string;
  browser_download_url?: string;
}
interface ReleaseData {
  tag_name?: string;
  assets?: ReleaseAsset[];
}

async function fetchRelease(version: string | undefined): Promise<ReleaseData> {
  const url =
    version === undefined
      ? GITHUB_API_LATEST
      : GITHUB_API_TAG(version.startsWith("v") ? version : `v${version}`);
  const response = await undiciFetch(url, {
    headers: { "User-Agent": USER_AGENT, Accept: "application/vnd.github+json" },
  });
  if (!response.ok) {
    throw new ZLayerInstallError(
      `GitHub returned HTTP ${response.status} for ${url}: ${response.statusText}`,
    );
  }
  return (await response.json()) as ReleaseData;
}

function resolveAssetUrl(
  release: ReleaseData,
  version: string,
  tag: string,
): string {
  const suffix = artifactSuffix();
  const candidates = [
    `${BINARY_NAME}-${version}-${suffix}.tar.gz`,
    `zlayer-${version}-${suffix}.tar.gz`,
  ];
  for (const candidate of candidates) {
    for (const asset of release.assets ?? []) {
      if (asset.name === candidate && asset.browser_download_url) {
        return asset.browser_download_url;
      }
    }
  }
  return GITHUB_DOWNLOAD(tag, candidates[0]!);
}

// ---------------------------------------------------------------------------
// Download + extract
// ---------------------------------------------------------------------------

async function downloadToFile(url: string, dest: string): Promise<string> {
  const response = await undiciFetch(url, {
    headers: { "User-Agent": USER_AGENT },
  });
  if (!response.ok || !response.body) {
    throw new ZLayerInstallError(
      `Download failed for ${url}: HTTP ${response.status} ${response.statusText}`,
    );
  }
  const hasher = createHash("sha256");
  const file = createWriteStream(dest);
  const { Readable } = await import("node:stream");
  const body = Readable.fromWeb(response.body as unknown as import("node:stream/web").ReadableStream);
  body.on("data", (chunk: Buffer) => hasher.update(chunk));
  await pipeline(body, file);
  return hasher.digest("hex");
}

/**
 * Extract the `zlayer` binary from a `.tar.gz` archive into `destDir`.
 *
 * We use `tar -xzf` via the system tar for reliability and to avoid
 * pulling in an npm dependency. Both Linux and macOS ship with a capable
 * tar in the base system; Windows is already gated out by
 * {@link artifactSuffix}.
 */
async function extractBinary(archive: string, destDir: string): Promise<string> {
  try {
    await execFile("tar", ["-xzf", archive, "-C", destDir]);
  } catch (error) {
    throw new ZLayerInstallError(
      `Failed to extract archive ${archive}: ${(error as Error).message}`,
      error,
    );
  }

  const found = await findBinaryIn(destDir);
  if (!found) {
    throw new ZLayerInstallError(
      `Binary '${BINARY_NAME}' not found in archive ${archive}`,
    );
  }
  return found;
}

async function findBinaryIn(dir: string): Promise<string | null> {
  const entries = await fs.readdir(dir, { withFileTypes: true });
  for (const entry of entries) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) {
      const nested = await findBinaryIn(path);
      if (nested) return nested;
    } else if (entry.isFile() && entry.name === BINARY_NAME) {
      return path;
    }
  }
  return null;
}

// ---------------------------------------------------------------------------
// Version helpers / daemon install
// ---------------------------------------------------------------------------

async function runBinaryVersion(binary: string): Promise<string | null> {
  try {
    const { stdout } = await execFile(binary, ["--version"], { timeout: 10_000 });
    const trimmed = stdout.trim();
    return trimmed ? trimmed : null;
  } catch {
    return null;
  }
}

function versionMatches(current: string, wanted: string): boolean {
  const clean = wanted.replace(/^v/, "").trim();
  return Boolean(clean) && current.includes(clean);
}

async function runDaemonInstall(binary: string): Promise<void> {
  const cmd = [binary, "daemon", "install"];
  const banner =
    "\n" +
    "================================================================\n" +
    `  About to run: sudo ${cmd.join(" ")}\n` +
    "  This command requires administrator privileges to install\n" +
    "  the zlayer daemon system-wide (services, unit files, etc.).\n" +
    "  Your terminal will prompt for your sudo password.\n" +
    "================================================================\n";
  process.stderr.write(banner);

  // Stream I/O through to the user — we intentionally do NOT buffer so
  // sudo prompts show up live.
  await new Promise<void>((resolve, reject) => {
    const child = spawn(cmd[0]!, cmd.slice(1), { stdio: "inherit" });
    child.once("error", (err) => {
      reject(new ZLayerInstallError(`Could not execute ${binary}: ${err.message}`, err));
    });
    child.once("exit", (code) => {
      if (code === 0) resolve();
      else
        reject(
          new ZLayerInstallError(
            `'zlayer daemon install' failed with exit code ${code}`,
          ),
        );
    });
  });
}

