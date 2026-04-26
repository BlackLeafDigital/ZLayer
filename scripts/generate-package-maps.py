#!/usr/bin/env python3
"""
Generate package name mapping files from the Repology database dump.

Downloads the Repology PostgreSQL dump, loads it, queries for
ALL distro → Homebrew formula mappings, and outputs one JSON file per distro.

Requires: PostgreSQL, zstd, psql in PATH.
"""

import json
import os
import subprocess
import sys
import tempfile
import urllib.request
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
# Use these only for entries Repology can't resolve (no effname join with
# homebrew), or to force the unversioned canonical when Repology only surfaces
# versioned aliases. NEVER pin to a specific version here — pinning behind the
# user's back defeats the point of letting Homebrew (and the user's own formula
# dependencies) drive version selection. If a caller wants e.g. `openssl@3`
# they can ask for it explicitly; the unversioned apt input (`libssl-dev`,
# `python3`) means "current default", which apt itself uses.
OVERRIDES = {
    "libssl-dev":    "openssl",
    "libssl3":       "openssl",
    "openssl-dev":   "openssl",
    "python3":       "python",
    "python3-dev":   "python",
    "python3-devel": "python",
    "nodejs":        "node",
    "default-jdk":   "openjdk",
    "default-jre":   "openjdk",
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

    # Build the psql command — needs su-exec when running as root
    if os.getuid() == 0:
        psql_base = f"su-exec postgres psql -h {tmpdir} -p {port} -U repology -d repology -v ON_ERROR_STOP=0"
    else:
        psql_base = f"psql -h {tmpdir} -p {port} -U repology -d repology -v ON_ERROR_STOP=0"

    # Capture errors separately so we can diagnose failures
    errlog = os.path.join(tmpdir, "load_errors.log")
    # IMPORTANT: psql reads SQL from stdin, outputs results to stdout.
    # Redirect stdout to /dev/null (we don't need CREATE TABLE output),
    # capture stderr (errors/notices) to the log file.
    cmd = f"curl -sL {DUMP_URL} | zstd -d | {psql_base} > /dev/null 2>{errlog}"
    print(f"  $ {cmd[:120]}...")
    subprocess.run(cmd, shell=True)

    # Show first errors if any
    if os.path.exists(errlog):
        with open(errlog) as f:
            errors = f.read().strip()
        if errors:
            lines = errors.split("\n")
            # Show first 30 error lines
            print(f"  Load errors ({len(lines)} lines, showing first 30):")
            for line in lines[:30]:
                print(f"    {line}")
        else:
            print("  No errors during load.")
    print("  Dump loaded.")


def diagnose(tmpdir, port):
    """Print diagnostic info about the loaded database."""
    print("\nDiagnostics:")

    # List tables
    tables = psql(tmpdir, port,
                  "SELECT tablename FROM pg_tables WHERE schemaname='repology' ORDER BY tablename;",
                  user=superuser())
    print(f"  Tables: {tables.replace(chr(10), ', ')}")

    # Check packages table
    if "packages" in tables:
        count = psql(tmpdir, port, "SELECT count(*) FROM repology.packages;", user="postgres")
        print(f"  packages row count: {count}")

        cols = psql(tmpdir, port,
                    "SELECT column_name FROM information_schema.columns WHERE table_name='packages' ORDER BY ordinal_position;",
                    user=superuser())
        print(f"  packages columns: {cols.replace(chr(10), ', ')}")

        # Sample repos
        repos = psql(tmpdir, port,
                     "SELECT DISTINCT repo FROM repology.packages WHERE repo LIKE 'homebrew%' OR repo LIKE 'debian%' OR repo LIKE 'ubuntu%' OR repo LIKE 'alpine%' LIMIT 20;",
                     user=superuser())
        print(f"  Sample repos: {repos.replace(chr(10), ', ')}")

        # Sample homebrew entries
        sample = psql(tmpdir, port,
                      "SELECT effname, repo, visiblename FROM repology.packages WHERE repo='homebrew' LIMIT 5;",
                      user=superuser())
        print(f"  Sample homebrew: {sample}")
    else:
        print("  WARNING: packages table not found!")
        # Check if there's a different table
        all_tables = psql(tmpdir, port,
                          "SELECT tablename FROM pg_tables WHERE schemaname='repology';",
                          user=superuser())
        print(f"  Available tables: {all_tables}")


def fetch_homebrew_alias_index():
    """Fetch the Homebrew formula API once and build an alias→canonical map.

    Each entry maps a formula's `full_name`, every entry in `aliases`, and every
    entry in `oldnames` to the formula's canonical `full_name`. This lets us
    prefer the alias-canonical formula when Repology surfaces multiple matches
    for the same Linux package (e.g. `openssl@3.5` vs `openssl@3`, where
    `openssl@3.aliases` contains `openssl` and `openssl@3.6`).

    On failure (network/parse error) returns an empty dict; the tiebreak then
    degrades to the version-suffix heuristic in `pick_winner`.
    """
    print(f"Fetching {HOMEBREW_FORMULA_API}...")
    try:
        with urllib.request.urlopen(HOMEBREW_FORMULA_API, timeout=60) as resp:
            data = json.load(resp)
    except Exception as e:
        print(f"  WARN: failed to fetch homebrew API ({e}); alias-aware tiebreak disabled")
        return {}

    a2c = {}
    for f in data:
        canonical = f.get("full_name") or f.get("name")
        if not canonical:
            continue
        a2c[canonical] = canonical
        for alias in f.get("aliases", []) or []:
            a2c[alias] = canonical
        for old in f.get("oldnames", []) or []:
            a2c[old] = canonical
    print(f"  Indexed {len(a2c)} alias entries from {len(data)} formulas")
    return a2c


def pick_winner(candidates, linux_name, alias_to_canonical):
    """Choose the best Homebrew formula for a Linux package name.

    Tiebreak order:
      1. If the Linux name is itself a known Homebrew alias and that alias's
         canonical formula is among the candidates, use it. This catches cases
         like `openssl → openssl@3` (since `openssl@3.aliases = ['openssl', ...]`).
      2. Among candidates, prefer one whose name has no `@` (unversioned base
         formula like `node` over `node@24`).
      3. Among `@`-versioned candidates, prefer fewer dotted components
         (`openssl@3` over `openssl@3.5` over `openssl@3.5.6`).
      4. Lexicographic.
    """
    if not candidates:
        return None
    if len(candidates) == 1:
        return candidates[0]

    # Rule 1: alias-canonical match
    canonical = alias_to_canonical.get(linux_name)
    if canonical and canonical in candidates:
        return canonical

    # Rules 2-4: version-suffix preference, then lex
    def sort_key(name):
        if "@" not in name:
            return (0, len(name), name)
        suffix = name.split("@", 1)[1]
        return (1 + suffix.count("."), len(name), name)

    return sorted(candidates, key=sort_key)[0]


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

    Captures all name variants: visiblename, srcname, binname, and
    individual entries from the binnames array. When a Linux package has
    multiple Homebrew candidates (different versioned aliases sharing an
    `effname`), uses `pick_winner` to choose the canonical one rather than
    accepting whichever PostgreSQL emits first.
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
        if "|" in line:
            parts = line.split("|", 1)
            if len(parts) == 2 and parts[0].strip() and parts[1].strip():
                linux_name = parts[0].strip()
                brew_name = parts[1].strip()
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
    final per-shard file already reflects the override.

    Atomicity: each shard is written to `.tmp-{shard}.json` and renamed to
    `{shard}.json`. A crash mid-loop leaves prior shards intact and at most
    one tmp file behind, never a half-written shard.

    Returns the list of shard letters that were written (i.e. had at least
    one entry after merge with existing).
    """
    # Bucket new mappings by shard
    new_buckets = {s: {} for s in SHARDS}
    for name, brew in mappings.items():
        new_buckets[shard_key(name)][name] = brew
    # Apply OVERRIDES per-shard
    for name, brew in OVERRIDES.items():
        new_buckets[shard_key(name)][name] = brew

    written = []
    total_new = 0
    total_after = 0
    for shard in SHARDS:
        shard_path = os.path.join(distro_dir, f"{shard}.json")

        # Read existing entries for this shard so packages only grow, never shrink
        existing = {}
        if os.path.exists(shard_path):
            with open(shard_path) as f:
                existing = json.load(f).get("mappings", {})

        new_for_shard = new_buckets[shard]
        # Existing first, new second so new wins on conflict
        merged = {**existing, **new_for_shard}
        # Re-apply OVERRIDES so they always win even over existing on-disk data
        for name, brew in OVERRIDES.items():
            if shard_key(name) == shard:
                merged[name] = brew

        if not merged:
            # Nothing to write; remove any pre-existing empty shard so the
            # index reflects reality
            continue

        doc = {
            "metadata": {
                "generated_at": generated_at,
                "source": "repology-dump",
                "distro": distro_repo,
                "shard": shard,
                "total_mappings": len(merged),
            },
            "mappings": dict(sorted(merged.items())),
        }

        tmp_path = os.path.join(distro_dir, f".tmp-{shard}.json")
        with open(tmp_path, "w") as f:
            json.dump(doc, f, indent=2)
        os.rename(tmp_path, shard_path)

        written.append(shard)
        total_new += len(new_for_shard)
        total_after += len(merged)

    print(
        f"  {distro_repo}: {total_after} mappings across {len(written)} shards "
        f"({total_new} from this run)"
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

            # Fetch the Homebrew alias index once; reused across every distro.
            alias_to_canonical = fetch_homebrew_alias_index()

            # Get ALL distro repos that have homebrew equivalents
            print("\nFinding all distro repos with Homebrew mappings...")
            repos = get_all_distro_repos(sock_dir, port)
            print(f"  Found {len(repos)} repos")

            if not repos:
                print("ERROR: No repos found. Check diagnostics above.")
                sys.exit(1)

            # Extract and write per-repo mapping files
            print(f"\nExtracting mappings for {len(repos)} repos...")
            os.makedirs(OUTPUT_DIR, exist_ok=True)

            generated_at = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
            total = 0
            for repo in repos:
                mappings = extract_mappings(sock_dir, port, repo, alias_to_canonical)
                if mappings:
                    distro_dir = os.path.join(OUTPUT_DIR, repo)
                    os.makedirs(distro_dir, exist_ok=True)
                    shards_written, total_after = write_sharded_map_files(
                        distro_dir, repo, mappings, generated_at
                    )
                    write_distro_index(
                        distro_dir, repo, shards_written, total_after, generated_at
                    )
                    total += total_after

            print(f"\nTotal: {total} mappings across {len(repos)} repos")
            print("Done!")

        finally:
            print("\nCleaning up PostgreSQL...")
            stop_postgres(tmpdir)


if __name__ == "__main__":
    main()
