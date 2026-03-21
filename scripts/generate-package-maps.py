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
from datetime import datetime, timezone

sys.stdout.reconfigure(line_buffering=True)
sys.stderr.reconfigure(line_buffering=True)

DUMP_URL = "https://dumps.repology.org/repology-database-dump-latest.sql.zst"
HOMEBREW_REPO = "homebrew"

OUTPUT_DIR = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "docs", "maps"
)


def run(cmd, **kwargs):
    print(f"  $ {cmd if isinstance(cmd, str) else ' '.join(cmd)}")
    return subprocess.run(cmd, shell=isinstance(cmd, str), check=True, **kwargs)


def pg_cmd(tmpdir, port, cmd):
    """Build a psql/createuser/etc command with su-exec if root."""
    if os.getuid() == 0:
        return f"su-exec postgres {cmd}"
    return cmd


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

    # Extensions (run as postgres superuser)
    psql(tmpdir, port, "CREATE EXTENSION IF NOT EXISTS pg_trgm;",
         user="postgres", capture=False)

    try:
        run(pg_cmd(tmpdir, port,
                   f'psql -h {tmpdir} -p {port} -U postgres -d repology -c "CREATE EXTENSION IF NOT EXISTS libversion;"'),
            capture_output=True)
        print("  libversion extension loaded")
    except subprocess.CalledProcessError:
        print("  ERROR: libversion extension not available!")
        sys.exit(1)

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

    psql_cmd = pg_cmd(tmpdir, port,
                      f"psql -h {tmpdir} -p {port} -U repology -d repology -v ON_ERROR_STOP=0")
    cmd = f"curl -sL {DUMP_URL} | zstd -d | {psql_cmd} 2>&1 | tail -5"
    result = subprocess.run(cmd, shell=True, capture_output=True, text=True)
    print(f"  Last lines: {result.stdout.strip()}")
    print("  Dump loaded.")


def diagnose(tmpdir, port):
    """Print diagnostic info about the loaded database."""
    print("\nDiagnostics:")

    # List tables
    tables = psql(tmpdir, port,
                  "SELECT tablename FROM pg_tables WHERE schemaname='public' ORDER BY tablename;",
                  user="postgres")
    print(f"  Tables: {tables.replace(chr(10), ', ')}")

    # Check packages table
    if "packages" in tables:
        count = psql(tmpdir, port, "SELECT count(*) FROM packages;", user="postgres")
        print(f"  packages row count: {count}")

        cols = psql(tmpdir, port,
                    "SELECT column_name FROM information_schema.columns WHERE table_name='packages' ORDER BY ordinal_position;",
                    user="postgres")
        print(f"  packages columns: {cols.replace(chr(10), ', ')}")

        # Sample repos
        repos = psql(tmpdir, port,
                     "SELECT DISTINCT repo FROM packages WHERE repo LIKE 'homebrew%' OR repo LIKE 'debian%' OR repo LIKE 'ubuntu%' OR repo LIKE 'alpine%' LIMIT 20;",
                     user="postgres")
        print(f"  Sample repos: {repos.replace(chr(10), ', ')}")

        # Sample homebrew entries
        sample = psql(tmpdir, port,
                      "SELECT effname, repo, visiblename FROM packages WHERE repo='homebrew' LIMIT 5;",
                      user="postgres")
        print(f"  Sample homebrew: {sample}")
    else:
        print("  WARNING: packages table not found!")
        # Check if there's a different table
        all_tables = psql(tmpdir, port,
                          "SELECT tablename FROM pg_tables WHERE schemaname='public';",
                          user="postgres")
        print(f"  Available tables: {all_tables}")


def get_all_distro_repos(tmpdir, port):
    """Get all non-homebrew repos that have packages also in homebrew."""
    result = psql(tmpdir, port, """
        SELECT DISTINCT d.repo
        FROM packages d
        JOIN packages h ON d.effname = h.effname
        WHERE h.repo = 'homebrew'
        AND d.repo != 'homebrew'
        ORDER BY d.repo;
    """, user="postgres")

    if not result:
        return []
    return [r.strip() for r in result.split("\n") if r.strip()]


def extract_mappings(tmpdir, port, distro_repo):
    """Extract distro→homebrew mappings for a single repo."""
    result = psql_query(tmpdir, port, f"""
        SELECT DISTINCT d.visiblename, h.visiblename
        FROM packages d
        JOIN packages h ON d.effname = h.effname
        WHERE h.repo = 'homebrew'
        AND d.repo = '{distro_repo}'
        AND d.visiblename IS NOT NULL
        AND h.visiblename IS NOT NULL
        ORDER BY d.visiblename;
    """)

    mappings = {}
    for line in result.split("\n"):
        if "|" in line:
            parts = line.split("|", 1)
            if len(parts) == 2 and parts[0].strip() and parts[1].strip():
                linux_name = parts[0].strip()
                brew_name = parts[1].strip()
                if linux_name not in mappings:
                    mappings[linux_name] = brew_name
    return mappings


def write_map_file(filepath, distro_repo, mappings):
    now = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    doc = {
        "metadata": {
            "generated_at": now,
            "source": "repology-dump",
            "distro": distro_repo,
            "total_mappings": len(mappings),
        },
        "mappings": dict(sorted(mappings.items())),
    }
    os.makedirs(os.path.dirname(filepath), exist_ok=True)
    with open(filepath, "w") as f:
        json.dump(doc, f, indent=2)
    size_kb = os.path.getsize(filepath) / 1024
    print(f"  {distro_repo}: {len(mappings)} mappings ({size_kb:.1f} KB)")


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

            total = 0
            for repo in repos:
                mappings = extract_mappings(sock_dir, port, repo)
                if mappings:
                    filepath = os.path.join(OUTPUT_DIR, f"{repo}.json")
                    write_map_file(filepath, repo, mappings)
                    total += len(mappings)

            print(f"\nTotal: {total} mappings across {len(repos)} repos")
            print("Done!")

        finally:
            print("\nCleaning up PostgreSQL...")
            stop_postgres(tmpdir)


if __name__ == "__main__":
    main()
