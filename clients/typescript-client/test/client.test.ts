/**
 * Integration test for the @zlayer/client façade.
 *
 * Spawns a real `zlayer serve` daemon against a temporary Unix socket and
 * exercises a handful of read-only Client methods. Automatically skipped
 * when no `zlayer` binary can be found (in `$PATH`, `target/release/`, or
 * `target/debug/`), so the test is safe to run on any dev box without a
 * prior build step.
 */
import { spawn, type ChildProcess } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, beforeAll, describe, expect, it } from "vitest";

import { Client } from "../src/index.js";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "..", "..", "..");

function locateBinary(): string | null {
  const candidates = [
    join(repoRoot, "target", "release", "zlayer"),
    join(repoRoot, "target", "debug", "zlayer"),
  ];
  for (const path of candidates) {
    if (existsSync(path)) return path;
  }
  // Fall back to PATH.
  const pathEnv = process.env.PATH ?? "";
  for (const dir of pathEnv.split(":")) {
    if (!dir) continue;
    const candidate = join(dir, "zlayer");
    if (existsSync(candidate)) return candidate;
  }
  return null;
}

async function waitForSocket(path: string, timeoutMs = 15_000): Promise<void> {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    if (existsSync(path)) return;
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error(`Timed out waiting for socket ${path}`);
}

const binary = locateBinary();
const describeIfBinary = binary ? describe : describe.skip;

describeIfBinary("Client integration (requires zlayer binary)", () => {
  let workdir: string;
  let socketPath: string;
  let daemon: ChildProcess | null = null;

  beforeAll(async () => {
    workdir = mkdtempSync(join(tmpdir(), "zlayer-ts-test-"));
    const runDir = join(workdir, "run");
    const dataDir = join(workdir, "data");
    mkdirSync(runDir, { recursive: true });
    mkdirSync(dataDir, { recursive: true });
    socketPath = join(runDir, "zlayer.sock");

    daemon = spawn(
      binary!,
      [
        "--data-dir",
        dataDir,
        "serve",
        "--socket",
        socketPath,
        "--no-swagger",
      ],
      {
        stdio: ["ignore", "pipe", "pipe"],
        env: {
          ...process.env,
          RUST_LOG: "warn",
          // Force every derived dir (logs, run, containers) under dataDir so
          // the test works without write access to /var/log/zlayer etc.
          ZLAYER_DATA_DIR: dataDir,
          HOME: workdir,
        },
      },
    );

    // Surface daemon output when debugging a failing test.
    daemon.stdout?.on("data", (chunk: Buffer) => {
      if (process.env.ZLAYER_TEST_VERBOSE) {
        process.stderr.write(`[zlayer stdout] ${chunk.toString()}`);
      }
    });
    daemon.stderr?.on("data", (chunk: Buffer) => {
      if (process.env.ZLAYER_TEST_VERBOSE) {
        process.stderr.write(`[zlayer stderr] ${chunk.toString()}`);
      }
    });

    await waitForSocket(socketPath);
  }, 30_000);

  afterAll(() => {
    if (daemon && !daemon.killed) {
      daemon.kill("SIGTERM");
    }
    if (workdir) {
      try {
        rmSync(workdir, { recursive: true, force: true });
      } catch {
        // Best-effort cleanup.
      }
    }
  });

  it("ps() returns an array against a fresh daemon", async () => {
    const client = new Client({ socket: socketPath });
    const containers = await client.ps();
    expect(Array.isArray(containers)).toBe(true);
    // A fresh daemon with no deployments should have zero containers.
    expect(containers).toHaveLength(0);
  });

  it("deployments() returns an empty array on a fresh daemon", async () => {
    const client = new Client({ socket: socketPath });
    const deployments = await client.deployments();
    expect(Array.isArray(deployments)).toBe(true);
    expect(deployments).toHaveLength(0);
  });

  it("runtimes() returns at least one built-in template", async () => {
    const client = new Client({ socket: socketPath });
    const runtimes = await client.runtimes();
    expect(Array.isArray(runtimes)).toBe(true);
    // The builder ships templates for node/python/rust/go at minimum.
    expect(runtimes.length).toBeGreaterThan(0);
  });
});

// Always-present skip notice so a "no tests found" run is informative.
if (!binary) {
  describe("Client integration", () => {
    it.skip("skipped: no zlayer binary found in PATH or target/{debug,release}/", () => {
      // Intentionally empty.
    });
  });
}
