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


def extract_mappings(tmpdir, port, distro_repo):
    """Extract distro→homebrew mappings for a single repo."""
    result = psql_query(tmpdir, port, f"""
        SELECT DISTINCT d.visiblename, h.visiblename
        FROM repology.packages d
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
