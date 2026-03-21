#!/usr/bin/env python3
"""
Generate package name mapping files from the Repology database dump.

Downloads the Repology PostgreSQL dump, loads it, queries for
Linux distro → Homebrew formula mappings, and outputs JSON files.

Requires: PostgreSQL, zstd, psql in PATH.

Usage:
    python3 scripts/generate-package-maps.py
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

OUTPUT_DIR = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "docs", "maps"
)

DISTRO_REPOS = {
    "debian": "debian_12",
    "ubuntu": "ubuntu_24_04",
    "alpine": "alpine_3_20",
}

HOMEBREW_REPO = "homebrew"

# SQL query to extract mappings for a given distro repo
MAPPING_QUERY = """
COPY (
    SELECT DISTINCT
        dp.name AS linux_name,
        hp.name AS brew_formula
    FROM packages dp
    JOIN packages hp ON dp.effname = hp.effname
    WHERE hp.repo = '{homebrew_repo}'
    AND dp.repo = '{distro_repo}'
    AND dp.name IS NOT NULL
    AND hp.name IS NOT NULL
    ORDER BY dp.name
) TO STDOUT WITH (FORMAT csv, HEADER false);
"""


def run(cmd, **kwargs):
    """Run a command, printing it first."""
    print(f"  $ {cmd if isinstance(cmd, str) else ' '.join(cmd)}")
    return subprocess.run(cmd, shell=isinstance(cmd, str), check=True, **kwargs)


def setup_postgres(tmpdir):
    """Initialize a temporary PostgreSQL instance."""
    pgdata = os.path.join(tmpdir, "pgdata")
    print("Setting up temporary PostgreSQL...")

    run(f"initdb -D {pgdata} --auth=trust --no-locale -E UTF8",
        capture_output=True)

    # Start PostgreSQL on a random port
    port = 15432
    logfile = os.path.join(tmpdir, "pg.log")
    run(f"pg_ctl -D {pgdata} -l {logfile} -o '-p {port} -k {tmpdir}' start",
        capture_output=True)

    # Create repology user and database
    run(f"createuser -h {tmpdir} -p {port} repology", capture_output=True)
    run(f"createdb -h {tmpdir} -p {port} -O repology repology", capture_output=True)

    # Create required extensions
    run(f'psql -h {tmpdir} -p {port} -d repology -c "CREATE EXTENSION IF NOT EXISTS pg_trgm;"',
        capture_output=True)

    # Try to create libversion extension (may not be available)
    try:
        run(f'psql -h {tmpdir} -p {port} -d repology -c "CREATE EXTENSION IF NOT EXISTS libversion;"',
            capture_output=True)
    except subprocess.CalledProcessError:
        print("  Warning: libversion extension not available, continuing without it")

    return tmpdir, port


def stop_postgres(tmpdir, pgdata=None):
    """Stop the temporary PostgreSQL instance."""
    if pgdata is None:
        pgdata = os.path.join(tmpdir, "pgdata")
    try:
        run(f"pg_ctl -D {pgdata} stop -m fast", capture_output=True)
    except subprocess.CalledProcessError:
        pass


def load_dump(tmpdir, port):
    """Download and load the Repology database dump."""
    print(f"Downloading Repology dump from {DUMP_URL}...")
    print("  (This may take a few minutes...)")

    # Stream: curl → zstd -d → psql
    cmd = (
        f"curl -sL {DUMP_URL} | zstd -d | "
        f"psql -h {tmpdir} -p {port} -U repology -d repology "
        f"-v ON_ERROR_STOP=0 2>&1 | tail -5"
    )
    result = subprocess.run(cmd, shell=True, capture_output=True, text=True)
    print(f"  Load output: {result.stdout.strip()}")
    if result.stderr:
        print(f"  Stderr: {result.stderr.strip()[:200]}")
    print("  Dump loaded.")


def extract_mappings(tmpdir, port, distro_repo):
    """Extract Linux→Homebrew mappings for a distro from the database."""
    query = MAPPING_QUERY.format(
        homebrew_repo=HOMEBREW_REPO,
        distro_repo=distro_repo,
    )

    result = subprocess.run(
        f'psql -h {tmpdir} -p {port} -U repology -d repology -t -A',
        shell=True,
        input=query,
        capture_output=True,
        text=True,
    )

    mappings = {}
    for line in result.stdout.strip().split("\n"):
        if "," in line:
            parts = line.split(",", 1)
            if len(parts) == 2 and parts[0] and parts[1]:
                linux_name, brew_formula = parts
                if linux_name not in mappings:
                    mappings[linux_name] = brew_formula

    return mappings


def write_map_file(filepath, distro_repo, mappings):
    """Write a single mapping JSON file."""
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
    print(f"  Wrote {filepath} ({size_kb:.1f} KB, {len(mappings)} mappings)")


def write_all_file(filepath, distro_maps):
    """Write the combined all.json file."""
    now = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    combined = {}
    for distro_key, mappings in distro_maps.items():
        combined[distro_key] = {
            "distro": DISTRO_REPOS[distro_key],
            "mappings": dict(sorted(mappings.items())),
            "total_mappings": len(mappings),
        }

    total = sum(len(m) for m in distro_maps.values())
    doc = {
        "metadata": {
            "generated_at": now,
            "source": "repology-dump",
            "distros": list(DISTRO_REPOS.values()),
            "total_mappings": total,
        },
        "distros": combined,
    }
    os.makedirs(os.path.dirname(filepath), exist_ok=True)
    with open(filepath, "w") as f:
        json.dump(doc, f, indent=2)
    size_kb = os.path.getsize(filepath) / 1024
    print(f"  Wrote {filepath} ({size_kb:.1f} KB, {total} total mappings)")


def main():
    print("=" * 60)
    print("RepoSources - Package Map Generator (dump method)")
    print("=" * 60)

    with tempfile.TemporaryDirectory(prefix="repology-") as tmpdir:
        try:
            # Set up PostgreSQL
            sock_dir, port = setup_postgres(tmpdir)
            print(f"PostgreSQL running on port {port}")

            # Load the dump
            load_dump(sock_dir, port)

            # Extract per-distro mappings
            distro_maps = {}
            for distro_key, distro_repo in DISTRO_REPOS.items():
                print(f"\nExtracting mappings for {distro_key} ({distro_repo})...")
                mappings = extract_mappings(sock_dir, port, distro_repo)
                distro_maps[distro_key] = mappings
                print(f"  {len(mappings)} mappings found")

            # Write output files
            print("\nWriting output files...")
            for distro_key, mappings in distro_maps.items():
                filepath = os.path.join(OUTPUT_DIR, f"{distro_key}.json")
                write_map_file(filepath, DISTRO_REPOS[distro_key], mappings)

            all_filepath = os.path.join(OUTPUT_DIR, "all.json")
            write_all_file(all_filepath, distro_maps)

            print("\nDone!")

        finally:
            print("\nCleaning up PostgreSQL...")
            stop_postgres(tmpdir)


if __name__ == "__main__":
    main()
