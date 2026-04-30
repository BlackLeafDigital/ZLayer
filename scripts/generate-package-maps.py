#!/usr/bin/env python3
"""
Generate package name mapping files from the Repology database dump.

Downloads the Repology PostgreSQL dump, loads it, queries for
ALL distro → Homebrew formula mappings, and outputs one JSON file per distro.

Requires: PostgreSQL, zstd, psql in PATH.
"""

import json
import os
import re
import subprocess
import sys
import tempfile
import urllib.request
from collections import Counter
from datetime import datetime, timezone

sys.stdout.reconfigure(line_buffering=True)
sys.stderr.reconfigure(line_buffering=True)

DUMP_URL = "https://dumps.repology.org/repology-database-dump-latest.sql.zst"
HOMEBREW_REPO = "homebrew"
HOMEBREW_FORMULA_API = "https://formulae.brew.sh/api/formula.json"

OUTPUT_DIR = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "public", "maps"
)

# Hand-curated overrides applied last, overriding anything Repology produces.
# Reserved for entries the dynamic `pick_winner` resolution genuinely cannot
# reach: no Repology effname join, ambiguous semantics, or distro-specific
# metapackages with no direct brew twin.
#
# Common version-mapping cases (libssl-dev -> openssl@3, python3 -> python@3.14,
# nodejs -> node@22) are handled by `pick_winner` using each Linux package's
# Repology version field plus Homebrew's formula list — DO NOT add them here.
OVERRIDES = {
    # Debian metapackages that point at "the distro's chosen default JDK/JRE".
    # Repology has no effname join for these; map by hand to brew's openjdk
    # (which itself tracks the current default).
    "default-jdk": "openjdk",
    "default-jre": "openjdk",
}

# Shard set used to split large per-distro maps into reviewable chunks.
# `_misc` catches anything whose first character isn't an ASCII a-z letter
# (digits, plus signs, etc.).
SHARDS = [chr(c) for c in range(ord("a"), ord("z") + 1)] + ["_misc"]


def shard_key(name: str) -> str:
    """Return the shard letter for a Linux package name.

    Lowercase first ASCII letter for `a`-`z` names; `_misc` for anything else
    (digits, `+`, etc.). The resolver uses the same function — keep them in
    sync.
    """
    if not name:
        return "_misc"
    first = name[0].lower()
    if "a" <= first <= "z":
        return first
    return "_misc"


def run(cmd, **kwargs):
    print(f"  $ {cmd if isinstance(cmd, str) else ' '.join(cmd)}")
    return subprocess.run(cmd, shell=isinstance(cmd, str), check=True, **kwargs)


def pg_cmd(tmpdir, port, cmd):
    """Build a psql/createuser/etc command with su-exec if root."""
    if os.getuid() == 0:
        return f"su-exec postgres {cmd}"
    return cmd


def superuser():
    """Return the PostgreSQL superuser name (postgres in containers, current user on macOS)."""
    if os.getuid() == 0:
        return "postgres"
    import getpass
    return getpass.getuser()


def psql(tmpdir, port, sql, db="repology", user="repology", capture=True):
    """Run a SQL query via psql, return stdout."""
    cmd = pg_cmd(tmpdir, port,
                 f"psql -h {tmpdir} -p {port} -U {user} -d {db} -t -A -c \"{sql}\"")
    result = subprocess.run(cmd, shell=True, capture_output=capture, text=True)
    return result.stdout.strip() if capture else ""


def psql_query(tmpdir, port, sql, db="repology", user="repology"):
    """Run a SQL query via psql with stdin, return stdout."""
    base = f"psql -h {tmpdir} -p {port} -U {user} -d {db} -t -A"
    cmd = pg_cmd(tmpdir, port, base)
    result = subprocess.run(cmd, shell=True, input=sql, capture_output=True, text=True)
    return result.stdout.strip()


def setup_postgres(tmpdir):
    pgdata = os.path.join(tmpdir, "pgdata")
    print("Setting up temporary PostgreSQL...")

    if os.getuid() == 0:
        run(f"chown -R postgres:postgres {tmpdir}", capture_output=True)

    run(pg_cmd(tmpdir, 0, f"initdb -D {pgdata} --auth=trust --no-locale -E UTF8"),
        capture_output=True)

    port = 15432
    logfile = os.path.join(tmpdir, "pg.log")
    run(pg_cmd(tmpdir, port,
               f"pg_ctl -D {pgdata} -l {logfile} -o '-p {port} -k {tmpdir}' start"),
        capture_output=True)

    run(pg_cmd(tmpdir, port, f"createuser -h {tmpdir} -p {port} repology"),
        capture_output=True)
    run(pg_cmd(tmpdir, port, f"createdb -h {tmpdir} -p {port} -O repology repology"),
        capture_output=True)

    # Pre-create the `repology` schema. The Repology dump references tables as
    # `repology.<name>` but doesn't ship a `CREATE SCHEMA` statement, so without
    # this every CREATE TABLE in the dump fails (silently, when ON_ERROR_STOP
    # is off). AUTHORIZATION repology lets the dump-loading user own the
    # objects it creates.
    psql(tmpdir, port,
         "CREATE SCHEMA IF NOT EXISTS repology AUTHORIZATION repology;",
         user=superuser(), capture=False)

    # Set search path to include repology schema
    psql(tmpdir, port, "ALTER DATABASE repology SET search_path TO repology, public;",
         user=superuser(), capture=False)

    # Extensions (run as superuser)
    su = superuser()
    psql(tmpdir, port, "CREATE EXTENSION IF NOT EXISTS pg_trgm;",
         user=su, capture=False)

    try:
        run(pg_cmd(tmpdir, port,
                   f'psql -h {tmpdir} -p {port} -U {su} -d repology -c "CREATE EXTENSION IF NOT EXISTS libversion;"'),
            capture_output=True)
        print("  libversion extension loaded")
    except subprocess.CalledProcessError:
        print("  WARNING: libversion extension not available, will attempt dump without it")
        print("  (Some tables may fail to load)")

    return tmpdir, port


def stop_postgres(tmpdir):
    pgdata = os.path.join(tmpdir, "pgdata")
    try:
        run(pg_cmd(tmpdir, 0, f"pg_ctl -D {pgdata} stop -m fast"), capture_output=True)
    except subprocess.CalledProcessError:
        pass


def load_dump(tmpdir, port):
    print(f"Downloading and loading Repology dump...")
    print("  (This may take several minutes...)")

    # Build the psql command — needs su-exec when running as root.
    # ON_ERROR_STOP=1: bail at the first SQL error rather than silently
    # skipping the rest of the dump and producing an empty database.
    if os.getuid() == 0:
        psql_base = f"su-exec postgres psql -h {tmpdir} -p {port} -U repology -d repology -v ON_ERROR_STOP=1"
    else:
        psql_base = f"psql -h {tmpdir} -p {port} -U repology -d repology -v ON_ERROR_STOP=1"

    # Capture errors separately so we can diagnose failures.
    errlog = os.path.join(tmpdir, "load_errors.log")
    curl_errlog = os.path.join(tmpdir, "curl_errors.log")
    zstd_errlog = os.path.join(tmpdir, "zstd_errors.log")

    # Run via `sh -o pipefail` so a curl/zstd failure (e.g. truncated
    # download, 4xx HTTP, malformed zstd frame) propagates as a non-zero
    # exit. Without pipefail the shell only reports psql's status, which
    # is "success" when psql cleanly drains a truncated stream.
    # busybox ash (alpine container) and bash (mac dev) both support
    # `-o pipefail`, so `sh` works in both environments.
    # `curl -fL --show-error`: -f turns 4xx/5xx into a non-zero exit
    # instead of writing the HTML error body into the zstd pipe.
    # Each command's stderr goes to its own log so we can attribute
    # failures correctly.
    inner = (
        f"curl -fL --show-error {DUMP_URL} 2>{curl_errlog} "
        f"| zstd -d 2>{zstd_errlog} "
        f"| {psql_base} > /dev/null 2>{errlog}"
    )
    cmd = ["sh", "-o", "pipefail", "-c", inner]
    print(f"  $ (pipefail) curl ... | zstd -d | psql ...")
    result = subprocess.run(cmd)

    # Print stderr from each pipeline stage so the failure is attributable.
    for label, path in [("curl", curl_errlog), ("zstd", zstd_errlog), ("psql", errlog)]:
        if not os.path.exists(path):
            continue
        with open(path) as f:
            errs = f.read().strip()
        if not errs:
            continue
        lines = errs.split("\n")
        print(f"  {label} stderr ({len(lines)} lines, showing first 30):")
        for line in lines[:30]:
            print(f"    {line}")

    if result.returncode != 0:
        raise SystemExit(
            f"ERROR: dump load failed (pipefail exit {result.returncode}). "
            f"See per-stage stderr above."
        )
    print("  Dump loaded.")


def diagnose(tmpdir, port):
    """Print diagnostic info about the loaded database.

    Raises SystemExit if the `packages` table is missing or empty — that's
    always a setup bug (schema not created, dump truncated, auth failure)
    and there's no point in continuing to "Found 0 repos".
    """
    print("\nDiagnostics:")
    su = superuser()

    # List schemas first so missing-schema bugs are obvious.
    schemas = psql(tmpdir, port,
                   "SELECT nspname FROM pg_namespace WHERE nspname NOT LIKE 'pg_%' AND nspname != 'information_schema' ORDER BY nspname;",
                   user=su)
    print(f"  Schemas: {schemas.replace(chr(10), ', ')}")

    # List tables
    tables = psql(tmpdir, port,
                  "SELECT tablename FROM pg_tables WHERE schemaname='repology' ORDER BY tablename;",
                  user=su)
    print(f"  Tables: {tables.replace(chr(10), ', ')}")

    if "packages" not in tables:
        all_tables = psql(tmpdir, port,
                          "SELECT schemaname || '.' || tablename FROM pg_tables WHERE schemaname NOT IN ('pg_catalog','information_schema') ORDER BY 1;",
                          user=su)
        print(f"  All non-system tables: {all_tables}")
        raise SystemExit(
            "ERROR: repology.packages table not found after dump load. "
            "Check the per-stage stderr above (psql/zstd/curl) — a truncated "
            "dump or missing schema is the usual cause."
        )

    count = psql(tmpdir, port, "SELECT count(*) FROM repology.packages;", user=su)
    print(f"  packages row count: {count}")
    try:
        if int(count) == 0:
            raise SystemExit("ERROR: repology.packages is empty after dump load.")
    except ValueError:
        raise SystemExit(f"ERROR: could not parse packages row count: {count!r}")

    cols = psql(tmpdir, port,
                "SELECT column_name FROM information_schema.columns WHERE table_name='packages' ORDER BY ordinal_position;",
                user=su)
    print(f"  packages columns: {cols.replace(chr(10), ', ')}")

    # Sample repos
    repos = psql(tmpdir, port,
                 "SELECT DISTINCT repo FROM repology.packages WHERE repo LIKE 'homebrew%' OR repo LIKE 'debian%' OR repo LIKE 'ubuntu%' OR repo LIKE 'alpine%' LIMIT 20;",
                 user=su)
    print(f"  Sample repos: {repos.replace(chr(10), ', ')}")

    # Sample homebrew entries
    sample = psql(tmpdir, port,
                  "SELECT effname, repo, visiblename FROM repology.packages WHERE repo='homebrew' LIMIT 5;",
                  user=su)
    print(f"  Sample homebrew: {sample}")


def fetch_homebrew_alias_index():
    """Fetch the Homebrew formula API and build the alias→canonical index.

    Returns a dict mapping every formula's `full_name`, every `aliases`
    entry, and every `oldnames` entry to that formula's canonical
    `full_name`. Canonical names map to themselves, so a single lookup
    answers both "is this a known brew name" and "what's its canonical".

    Used by `pick_winner` to:
      - Canonicalize candidates (collapse `[openssl, openssl@3]` to
        `[openssl@3]` because brew aliases `openssl` to `openssl@3`).
      - Resolve the Linux package name to its brew canonical alias
        (e.g. `python3` → `python@3.14`).

    On failure (network/parse error) returns `{}`; `pick_winner` degrades
    to the lex/versioned-highest tiebreak path.
    """
    print(f"Fetching {HOMEBREW_FORMULA_API}...")
    try:
        with urllib.request.urlopen(HOMEBREW_FORMULA_API, timeout=60) as resp:
            data = json.load(resp)
    except Exception as e:
        print(f"  WARN: failed to fetch homebrew API ({e}); alias index empty")
        return {}

    a2c = {}
    formula_count = 0
    for f in data:
        canonical = f.get("full_name") or f.get("name")
        if not canonical:
            continue
        formula_count += 1
        a2c[canonical] = canonical
        for alias in f.get("aliases", []) or []:
            a2c[alias] = canonical
        for old in f.get("oldnames", []) or []:
            a2c[old] = canonical
    print(f"  Indexed {len(a2c)} alias entries from {formula_count} formulas")
    return a2c


_LEADING_INT_RE = re.compile(r"^(\d+)")


def parse_major(version_or_suffix):
    """Extract the leading integer from a version string or `@`-suffix.

    Examples: `'3.0.16+5'` -> 3, `'22.5.1~deb12'` -> 22, `'3.14.0'` -> 3,
    `''`/`None`/`'foo'` -> None. Used to compare a Linux package's version
    against a brew formula's `@MAJOR` suffix.
    """
    if not version_or_suffix:
        return None
    m = _LEADING_INT_RE.match(version_or_suffix)
    return int(m.group(1)) if m else None


def pick_winner(candidates, linux_name, alias_to_canonical):
    """Choose the best Homebrew formula for a Linux package name.

    Algorithm (deterministic; no version-pin overrides needed):

    Step 1 — **Canonicalize candidates, dedupe.** Each brew name (alias or
        canonical) is mapped through `alias_to_canonical` to its full_name.
        Collapses `[openssl, openssl@3]` -> `[openssl@3]` because brew's
        `openssl` is an alias for `openssl@3`. If a single canonical
        remains, it's the answer.

    Step 2 — **Linux name as brew alias.** If the Linux name (or its
        `-dev`/`-devel`-stripped form) is itself a brew alias whose
        canonical is among the canonicals, return that canonical. There is
        NO `linux_major == canonical_major` check — brew's current is
        what users want regardless of how outdated the source distro is.

    Step 3 — **Versioned-preference tiebreaker.** Folded into Step 2: if
        Step 2's canonical is unversioned (e.g. `nodejs` resolves to
        `node`, but brew has both `node` AND `node@24`), prefer the
        highest pure-`@MAJOR` variant when it's in canonicals.

    Step 4 — **Final tiebreak among canonicals.** Prefer versioned
        (highest @MAJOR) over unversioned, else lex-first.
    """
    if not candidates:
        return None
    if len(candidates) == 1:
        return candidates[0]

    canonicals = []
    seen = set()
    for cand in candidates:
        c = alias_to_canonical.get(cand, cand)
        if c not in seen:
            seen.add(c)
            canonicals.append(c)
    if len(canonicals) == 1:
        return canonicals[0]

    alias_variants = [linux_name]
    for suffix in ("-dev", "-devel"):
        if linux_name.endswith(suffix):
            alias_variants.append(linux_name[: -len(suffix)])

    for variant in alias_variants:
        canonical = alias_to_canonical.get(variant)
        if not canonical or canonical not in canonicals:
            continue
        if "@" not in canonical:
            prefix = canonical + "@"
            versioned = [
                c for c in canonicals
                if c.startswith(prefix) and c[len(prefix):].isdigit()
            ]
            if versioned:
                return max(versioned, key=lambda c: int(c.rsplit("@", 1)[1]))
        return canonical

    versioned = [c for c in canonicals if "@" in c]
    if versioned:
        return max(
            versioned,
            key=lambda c: (parse_major(c.rsplit("@", 1)[1]) or 0, c),
        )
    return sorted(canonicals)[0]


def get_all_distro_repos(tmpdir, port):
    """Get all non-homebrew repos that have packages also in homebrew."""
    result = psql(tmpdir, port, """
        SELECT DISTINCT d.repo
        FROM repology.packages d
        JOIN packages h ON d.effname = h.effname
        WHERE h.repo = 'homebrew'
        AND d.repo != 'homebrew'
        ORDER BY d.repo;
    """, user="postgres")

    if not result:
        return []
    return [r.strip() for r in result.split("\n") if r.strip()]


def extract_mappings(tmpdir, port, distro_repo, alias_to_canonical):
    """Extract distro→homebrew mappings for a single repo.

    Captures all name variants (visiblename, srcname, binname, binnames[])
    and joins them to homebrew's effname. `pick_winner` reduces the
    resulting candidate list to a single canonical brew formula.
    """
    result = psql_query(tmpdir, port, f"""
        SELECT DISTINCT d_name, h.visiblename
        FROM (
            SELECT effname, visiblename AS d_name
            FROM repology.packages WHERE repo = '{distro_repo}'
            UNION
            SELECT effname, srcname
            FROM repology.packages WHERE repo = '{distro_repo}' AND srcname IS NOT NULL
            UNION
            SELECT effname, binname
            FROM repology.packages WHERE repo = '{distro_repo}' AND binname IS NOT NULL
            UNION
            SELECT effname, unnest(binnames)
            FROM repology.packages WHERE repo = '{distro_repo}' AND binnames IS NOT NULL
        ) d
        JOIN repology.packages h ON d.effname = h.effname
        WHERE h.repo = 'homebrew'
        AND d_name IS NOT NULL
        AND h.visiblename IS NOT NULL
        ORDER BY d_name;
    """)

    candidates = {}
    for line in result.split("\n"):
        parts = line.split("|", 1)
        if len(parts) != 2:
            continue
        linux_name = parts[0].strip()
        brew_name = parts[1].strip()
        if not linux_name or not brew_name:
            continue

        bucket = candidates.setdefault(linux_name, [])
        if brew_name not in bucket:
            bucket.append(brew_name)

    return {
        linux_name: pick_winner(cands, linux_name, alias_to_canonical)
        for linux_name, cands in candidates.items()
    }


def write_sharded_map_files(distro_dir, distro_repo, mappings, generated_at):
    """Write per-shard JSON files under {distro_dir}/{shard}.json.

    Each shard contains only the entries whose first letter belongs to that
    shard. OVERRIDES are applied to the shard their key belongs to so the
    final per-shard file reflects the override.

    Each run authoritatively overwrites the previous run's shards — there
    is NO merge with on-disk data. Per-distro shrinkage from one run to
    the next is acceptable because the cross-distro `common/` consensus
    preserves any name still produced by ANY distro. Eliminates zombie
    entries (e.g. centos_8 retaining `libssl-dev: openssl` from when an
    older OVERRIDES applied to all distros uniformly).

    Atomicity: each shard is written to `.tmp-{shard}.json` and renamed to
    `{shard}.json`. A crash mid-loop leaves prior shards intact and at most
    one tmp file behind, never a half-written shard.

    Returns `(shards_written, total_mappings)` for the caller's index.
    """
    new_buckets = {s: {} for s in SHARDS}
    for name, brew in mappings.items():
        new_buckets[shard_key(name)][name] = brew
    for name, brew in OVERRIDES.items():
        new_buckets[shard_key(name)][name] = brew

    written = []
    total_after = 0
    for shard in SHARDS:
        shard_path = os.path.join(distro_dir, f"{shard}.json")
        new_for_shard = new_buckets[shard]

        if not new_for_shard:
            # Authoritative empty: drop any stale on-disk file for this shard
            # so the index reflects reality.
            if os.path.exists(shard_path):
                os.remove(shard_path)
            continue

        doc = {
            "metadata": {
                "generated_at": generated_at,
                "source": "repology-dump",
                "distro": distro_repo,
                "shard": shard,
                "total_mappings": len(new_for_shard),
            },
            "mappings": dict(sorted(new_for_shard.items())),
        }

        tmp_path = os.path.join(distro_dir, f".tmp-{shard}.json")
        with open(tmp_path, "w") as f:
            json.dump(doc, f, indent=2)
        os.rename(tmp_path, shard_path)

        written.append(shard)
        total_after += len(new_for_shard)

    print(
        f"  {distro_repo}: {total_after} mappings across {len(written)} shards"
    )
    return written, total_after


def write_distro_index(distro_dir, distro_repo, shards_written, total_mappings, generated_at):
    """Write {distro_dir}/index.json after all shards are in place.

    Always called *after* `write_sharded_map_files` so a verifier reading
    `index.json` sees a complete shard set or the previous run's index.
    """
    doc = {
        "metadata": {
            "generated_at": generated_at,
            "source": "repology-dump",
            "distro": distro_repo,
            "shards": shards_written,
            "total_mappings": total_mappings,
        }
    }
    index_path = os.path.join(distro_dir, "index.json")
    tmp_path = os.path.join(distro_dir, ".tmp-index.json")
    with open(tmp_path, "w") as f:
        json.dump(doc, f, indent=2)
    os.rename(tmp_path, index_path)


def build_consensus(per_distro):
    """Compute the cross-distro consensus mapping from per-distro mappings.

    For each Linux name seen in any distro, collect the distinct brew
    values that distros mapped it to. The consensus rule:

      - **Single value across distros:** that's the answer.
      - **Multiple values:** most-common wins. Ties (multiple values share
        the top vote count) are broken by versioned-highest-major, then
        lex — same logic as `pick_winner`'s Step 4 tiebreak, so per-distro
        and consensus selection stay consistent.

    The output is the input to the `common/` shard set: the resolver
    falls back here when a per-distro shard doesn't have a name. This is
    how `apt install libssl-dev` (debian-only) still resolves on macOS
    when the source distro is centos/fedora — debian produces the
    mapping, consensus copies it, and resolver finds it via common.
    """
    union = {}
    for mappings in per_distro.values():
        for name, brew in mappings.items():
            union.setdefault(name, []).append(brew)

    consensus = {}
    for name, values in union.items():
        counts = Counter(values)
        top_count = counts.most_common(1)[0][1]
        leaders = [v for v, c in counts.items() if c == top_count]
        if len(leaders) == 1:
            consensus[name] = leaders[0]
            continue
        versioned = [c for c in leaders if "@" in c]
        if versioned:
            consensus[name] = max(
                versioned,
                key=lambda c: (parse_major(c.rsplit("@", 1)[1]) or 0, c),
            )
        else:
            consensus[name] = sorted(leaders)[0]
    return consensus


def main():
    print("=" * 60)
    print("RepoSources - Package Map Generator (dump method)")
    print("=" * 60)

    with tempfile.TemporaryDirectory(prefix="repology-") as tmpdir:
        try:
            sock_dir, port = setup_postgres(tmpdir)
            print(f"PostgreSQL running on port {port}")

            load_dump(sock_dir, port)
            diagnose(sock_dir, port)

            # Fetch the Homebrew alias index once; reused across every distro
            # by `pick_winner`.
            alias_to_canonical = fetch_homebrew_alias_index()

            # Get ALL distro repos that have homebrew equivalents
            print("\nFinding all distro repos with Homebrew mappings...")
            repos = get_all_distro_repos(sock_dir, port)
            print(f"  Found {len(repos)} repos")

            if not repos:
                print("ERROR: No repos found. Check diagnostics above.")
                sys.exit(1)

            os.makedirs(OUTPUT_DIR, exist_ok=True)
            generated_at = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")

            # Pass 1: extract per-distro mappings.
            print(f"\nExtracting mappings for {len(repos)} repos...")
            per_distro = {}
            for repo in repos:
                mappings = extract_mappings(
                    sock_dir, port, repo, alias_to_canonical
                )
                if mappings:
                    per_distro[repo] = mappings

            # Pass 2: build cross-distro consensus. For each Linux name seen
            # across distros, pick the single canonical brew formula. The
            # `common/` shard set is the resolver's cross-distro fallback —
            # it covers cases where one distro's source name doesn't exist on
            # another (e.g. `libssl-dev` on debian, `openssl-devel` on centos)
            # but the macOS resolver still wants a single answer.
            common_mappings = build_consensus(per_distro)

            # Pass 3: write per-distro shards.
            total = 0
            for repo, mappings in per_distro.items():
                distro_dir = os.path.join(OUTPUT_DIR, repo)
                os.makedirs(distro_dir, exist_ok=True)
                shards_written, total_after = write_sharded_map_files(
                    distro_dir, repo, mappings, generated_at
                )
                write_distro_index(
                    distro_dir, repo, shards_written, total_after, generated_at
                )
                total += total_after

            # Pass 4: write common shards.
            common_dir = os.path.join(OUTPUT_DIR, "common")
            os.makedirs(common_dir, exist_ok=True)
            common_shards, common_total = write_sharded_map_files(
                common_dir, "common", common_mappings, generated_at
            )
            write_distro_index(
                common_dir, "common", common_shards, common_total, generated_at
            )

            print(
                f"\nTotal: {total} per-distro mappings across {len(per_distro)} repos; "
                f"{common_total} mappings in common/"
            )
            print("Done!")

        finally:
            print("\nCleaning up PostgreSQL...")
            stop_postgres(tmpdir)


def _self_test():
    """Exercise pick_winner + build_consensus against synthetic inputs.

    Run with `python3 scripts/generate-package-maps.py --test`. No network,
    no Repology dump, no postgres. Catches regressions in the resolution
    rules without waiting for the ~10-minute live regenerator.
    """
    # Synthetic alias_to_canonical: matches what the brew API actually
    # returns today for these names (verified manually).
    a2c = {
        # openssl: bare alias -> versioned canonical
        "openssl": "openssl@3",
        "openssl@3": "openssl@3",
        "openssl@1.1": "openssl@1.1",
        # python: bare alias -> versioned canonical
        "python": "python@3.14",
        "python3": "python@3.14",
        "python@3.14": "python@3.14",
        "python@3.13": "python@3.13",
        # node: bare name is ITSELF canonical (versioned variants exist alongside)
        "node": "node",
        "nodejs": "node",
        "node@22": "node@22",
        "node@24": "node@24",
        # gcc: same shape as node
        "gcc": "gcc",
        "gcc@13": "gcc@13",
        "gcc@14": "gcc@14",
    }

    cases = [
        # Step 1 dedupe: openssl + openssl@3 -> openssl@3
        ("libssl-dev", ["openssl", "openssl@3"], "openssl@3"),
        # Step 2 alias-via-stripping; EOL openssl 1.0 must NOT veto openssl@3
        ("openssl-devel", ["openssl", "openssl@3", "openssl@1.1"], "openssl@3"),
        # Step 2 + Step 3 versioned-preference: nodejs alias -> node, node@24
        # is in canonicals -> pick node@24
        ("nodejs", ["node", "node@22", "node@24"], "node@24"),
        # Step 1 dedupe via python alias chain
        ("python3", ["python", "python@3.13", "python@3.14"], "python@3.14"),
        # Step 2 + Step 3 versioned-preference for gcc
        ("gcc", ["gcc", "gcc@13", "gcc@14"], "gcc@14"),
        # Single candidate short-circuit
        ("bash", ["bash"], "bash"),
        # Empty candidates -> None
        ("nonexistent", [], None),
    ]

    failures = []
    for linux_name, candidates, expected in cases:
        actual = pick_winner(candidates, linux_name, a2c)
        status = "OK  " if actual == expected else "FAIL"
        print(f"  {status} pick_winner({linux_name!r}, {candidates!r}) = {actual!r}  (expected {expected!r})")
        if actual != expected:
            failures.append((linux_name, candidates, expected, actual))

    # build_consensus: same name produced by multiple distros, possibly with
    # different brew values. Verifies cross-distro tie-breaking.
    print()
    consensus_cases = [
        # All distros agree -> that value
        (
            {"debian_12": {"libssl-dev": "openssl@3"}, "alpine_3_20": {"libssl-dev": "openssl@3"}},
            {"libssl-dev": "openssl@3"},
        ),
        # Single distro produces the name -> that value (no tie)
        (
            {"debian_12": {"libssl-dev": "openssl@3"}, "alpine_3_20": {}},
            {"libssl-dev": "openssl@3"},
        ),
        # Tie broken by versioned-highest
        (
            {
                "debian_12": {"python3": "python@3.14"},
                "alpine_3_20": {"python3": "python@3.13"},
            },
            {"python3": "python@3.14"},
        ),
        # Most-common beats versioned-highest
        (
            {
                "debian_12": {"node": "node@22"},
                "alpine_3_20": {"node": "node@22"},
                "centos_8": {"node": "node@24"},
            },
            {"node": "node@22"},
        ),
    ]

    for per_distro, expected in consensus_cases:
        actual = build_consensus(per_distro)
        status = "OK  " if actual == expected else "FAIL"
        print(f"  {status} build_consensus({per_distro!r}) = {actual!r}  (expected {expected!r})")
        if actual != expected:
            failures.append(("consensus", per_distro, expected, actual))

    print()
    if failures:
        print(f"FAIL: {len(failures)} case(s) failed.")
        sys.exit(1)
    print("All self-test cases passed.")


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "--test":
        _self_test()
    else:
        main()
