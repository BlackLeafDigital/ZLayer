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
    """Fetch the Homebrew formula API once and build the alias index + formula set.

    Returns `(alias_to_canonical, formula_set)`:

    - `alias_to_canonical`: maps every formula's `full_name`, every `aliases`
      entry, and every `oldnames` entry to that formula's canonical `full_name`.
      Used by `pick_winner` Rule 2 (resolve a Linux name like `python3` to the
      brew alias's current canonical, e.g. `python@3.14`).

    - `formula_set`: the set of canonical `full_name` values — i.e. the names
      that are *real* installable formulas, not just aliases. Used by
      `pick_winner` Rule 1 to verify that a synthesized `<base>@<major>` name
      actually exists in brew before emitting it (e.g. `openssl@3` is real,
      `python@3` is alias-only and would 404 on the formula JSON endpoint).

    On failure (network/parse error) returns `({}, set())`; `pick_winner`
    degrades to the version-suffix heuristic.
    """
    print(f"Fetching {HOMEBREW_FORMULA_API}...")
    try:
        with urllib.request.urlopen(HOMEBREW_FORMULA_API, timeout=60) as resp:
            data = json.load(resp)
    except Exception as e:
        print(f"  WARN: failed to fetch homebrew API ({e}); alias-aware tiebreak disabled")
        return {}, set()

    a2c = {}
    formulas = set()
    for f in data:
        canonical = f.get("full_name") or f.get("name")
        if not canonical:
            continue
        formulas.add(canonical)
        a2c[canonical] = canonical
        for alias in f.get("aliases", []) or []:
            a2c[alias] = canonical
        for old in f.get("oldnames", []) or []:
            a2c[old] = canonical
    print(f"  Indexed {len(a2c)} alias entries from {len(formulas)} formulas")
    return a2c, formulas


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


def parse_brew_major(brew_name):
    """Return the major from a brew name like `'openssl@3'` -> 3.

    Returns None for unversioned names (`'node'`) or unparseable suffixes
    (`'gcc@HEAD'`).
    """
    if "@" not in brew_name:
        return None
    return parse_major(brew_name.split("@", 1)[1])


def pick_winner(candidates, linux_name, linux_version, alias_to_canonical, formula_set):
    """Choose the best Homebrew formula for a Linux package name.

    Resolution order:
      1. **Real `<base>@<linux_major>` match.** If the Linux package's version
         starts with major `M` and any candidate is exactly `<base>@<M>`
         where that name is in `formula_set` (a real installable formula,
         not just an alias), return it. Example: `libssl-dev v3.0.16` →
         `openssl@3` (real formula).
      2. **Alias-canonical resolution.** If `linux_name` is itself a known
         brew alias and the canonical it points at is among the candidates,
         and (when we know the Linux major) the canonical's major matches,
         return it. Example: `python3 v3.13.0` → `python@3.14` (alias
         canonical of `python3`/`python@3`, major matches).
      3. **Version-suffix heuristic.** Prefer no-`@`, then fewer dotted
         components, then lex. Catches cases without version data or where
         brew's naming doesn't follow the `<base>@<major>` convention.
    """
    if not candidates:
        return None
    if len(candidates) == 1:
        return candidates[0]

    linux_major = parse_major(linux_version)

    # Rule 1: real `<base>@<linux_major>` formula in candidates. Match only
    # *pure* major pins — the suffix must be the integer with no dotted
    # minor/patch. Otherwise `python@3.10` would match a `python3 v3.x`
    # query (its leading int is 3) when what we actually want is the
    # alias-canonical `python@3.14`. Pure-major pins like `openssl@3` and
    # `node@22` match correctly.
    if linux_major is not None:
        for cand in candidates:
            if "@" not in cand:
                continue
            suffix = cand.split("@", 1)[1]
            if suffix.isdigit() and int(suffix) == linux_major and cand in formula_set:
                return cand

    # Rule 2: alias-canonical. The Linux name (`python3`, `openssl`) is
    # itself a brew alias — or one of its common Linux variants
    # (`python3-dev`, `python3-devel`). Try the name as-is, then with
    # `-dev`/`-devel` stripped so dev packages resolve to the same canonical
    # as their runtime counterpart. The Rust resolver only does name
    # transforms on map *misses*, so the generator must emit the right
    # value when the Linux name is a -dev variant.
    alias_variants = [linux_name]
    for suffix in ("-dev", "-devel"):
        if linux_name.endswith(suffix):
            alias_variants.append(linux_name[: -len(suffix)])

    for variant in alias_variants:
        canonical = alias_to_canonical.get(variant)
        if canonical and canonical in candidates:
            canonical_major = parse_brew_major(canonical)
            if (
                linux_major is None
                or canonical_major is None
                or canonical_major == linux_major
            ):
                return canonical

    # Rule 3: version-suffix preference, then lex
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


def extract_mappings(tmpdir, port, distro_repo, alias_to_canonical, formula_set):
    """Extract distro→homebrew mappings for a single repo.

    Captures all name variants (visiblename, srcname, binname, binnames[])
    along with the Linux package's `version` field — `pick_winner` uses the
    version's major to find the matching `<base>@<major>` brew formula.
    Per-(linux_name, brew_candidate) we keep the highest version seen (lex
    sort over Repology versions is good enough for major-extraction).
    """
    result = psql_query(tmpdir, port, f"""
        SELECT DISTINCT d_name, d_version, h.visiblename
        FROM (
            SELECT effname, visiblename AS d_name, version AS d_version
            FROM repology.packages WHERE repo = '{distro_repo}'
            UNION
            SELECT effname, srcname, version
            FROM repology.packages WHERE repo = '{distro_repo}' AND srcname IS NOT NULL
            UNION
            SELECT effname, binname, version
            FROM repology.packages WHERE repo = '{distro_repo}' AND binname IS NOT NULL
            UNION
            SELECT effname, unnest(binnames), version
            FROM repology.packages WHERE repo = '{distro_repo}' AND binnames IS NOT NULL
        ) d
        JOIN repology.packages h ON d.effname = h.effname
        WHERE h.repo = 'homebrew'
        AND d_name IS NOT NULL
        AND h.visiblename IS NOT NULL
        ORDER BY d_name;
    """)

    candidates = {}
    linux_versions = {}
    for line in result.split("\n"):
        # Each row: d_name|d_version|h.visiblename. Use maxsplit so version
        # strings containing `|` (rare but legal in Repology) don't corrupt
        # the brew name field.
        parts = line.split("|", 2)
        if len(parts) != 3:
            continue
        linux_name = parts[0].strip()
        linux_version = parts[1].strip()
        brew_name = parts[2].strip()
        if not linux_name or not brew_name:
            continue

        bucket = candidates.setdefault(linux_name, [])
        if brew_name not in bucket:
            bucket.append(brew_name)

        existing = linux_versions.get(linux_name)
        if linux_version and (not existing or linux_version > existing):
            linux_versions[linux_name] = linux_version

    return {
        linux_name: pick_winner(
            cands,
            linux_name,
            linux_versions.get(linux_name, ""),
            alias_to_canonical,
            formula_set,
        )
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

            # Fetch the Homebrew alias index + formula set once; reused
            # across every distro by `pick_winner`.
            alias_to_canonical, formula_set = fetch_homebrew_alias_index()

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
                mappings = extract_mappings(
                    sock_dir, port, repo, alias_to_canonical, formula_set
                )
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
