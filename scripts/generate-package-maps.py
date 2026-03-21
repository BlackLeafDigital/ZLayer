#!/usr/bin/env python3
"""
Generate package name mapping files from the Repology API.

Paginates through projects that exist in both Homebrew and various Linux distro
repositories, then outputs JSON mapping files (distro package name -> Homebrew
formula name) to docs/maps/.

Usage:
    python3 scripts/generate-maps.py
"""

import json
import os
import sys
import time
from datetime import datetime, timezone

import requests

API_BASE = "https://repology.org/api/v1/projects/"

# Repology repo identifiers
DISTRO_REPOS = {
    "debian": "debian_12",
    "ubuntu": "ubuntu_24_04",
    "alpine": "alpine_3_20",
}

HOMEBREW_REPO = "homebrew"

OUTPUT_DIR = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "docs", "maps")

# Rate limiting
REQUEST_DELAY = 1.0  # seconds between API requests
MAX_RETRIES = 5
INITIAL_BACKOFF = 2.0  # seconds, doubles on each retry

SESSION = requests.Session()
SESSION.headers.update({
    "User-Agent": "RepoSources/1.0 (https://github.com/ZachHandley/RepoSources; package mapping generator)",
})


def fetch_page(name_after=None):
    """Fetch one page of projects from Repology, with retry and backoff."""
    params = {
        "inrepo": HOMEBREW_REPO,
    }

    url = API_BASE
    if name_after:
        url = f"{API_BASE}?name_after={name_after}"
        # inrepo still needs to be passed
        params = {}
        url += f"&inrepo={HOMEBREW_REPO}"

    backoff = INITIAL_BACKOFF
    for attempt in range(1, MAX_RETRIES + 1):
        try:
            resp = SESSION.get(url if name_after else API_BASE, params=params if not name_after else None, timeout=30)

            if resp.status_code == 200:
                return resp.json()

            if resp.status_code in (403, 429, 500, 502, 503):
                wait = backoff * attempt
                print(f"  HTTP {resp.status_code} on attempt {attempt}/{MAX_RETRIES}, retrying in {wait:.0f}s...")
                time.sleep(wait)
                backoff *= 2
                continue

            # Other errors: log and return empty to skip this page
            print(f"  Unexpected HTTP {resp.status_code}, skipping page (attempt {attempt})")
            if attempt == MAX_RETRIES:
                return {}
            time.sleep(backoff)
            continue

        except requests.exceptions.RequestException as exc:
            wait = backoff * attempt
            print(f"  Request error on attempt {attempt}/{MAX_RETRIES}: {exc}, retrying in {wait:.0f}s...")
            time.sleep(wait)
            backoff *= 2
            if attempt == MAX_RETRIES:
                print("  Max retries reached, skipping page.")
                return {}

    return {}


def extract_mappings(projects_data, distro_repo):
    """
    Given a page of Repology project data, extract distro->homebrew mappings.

    Returns a dict of {distro_package_name: homebrew_formula_name}.
    """
    mappings = {}

    for project_name, packages in projects_data.items():
        homebrew_names = set()
        distro_names = set()

        for pkg in packages:
            repo = pkg.get("repo", "")
            name = pkg.get("srcname") or pkg.get("binname") or pkg.get("name", "")
            if not name:
                continue

            if repo == HOMEBREW_REPO:
                # Use visiblename for Homebrew since it reflects the actual formula
                visible = pkg.get("visiblename") or name
                homebrew_names.add(visible)
            elif repo == distro_repo:
                # Collect both srcname and binnames for distro packages
                if pkg.get("srcname"):
                    distro_names.add(pkg["srcname"])
                if pkg.get("binname"):
                    distro_names.add(pkg["binname"])
                if not pkg.get("srcname") and not pkg.get("binname"):
                    distro_names.add(name)

        # Create mappings: each distro name maps to the first homebrew name
        if homebrew_names and distro_names:
            brew_name = sorted(homebrew_names)[0]  # deterministic pick
            for dname in distro_names:
                if dname not in mappings:
                    mappings[dname] = brew_name

    return mappings


def paginate_all():
    """
    Paginate through the entire Repology API for Homebrew projects.

    Returns all project data as a combined dict.
    """
    all_projects = {}
    name_after = None
    page_num = 0

    print("Starting Repology API pagination...")
    print(f"Fetching projects present in {HOMEBREW_REPO}...")

    while True:
        page_num += 1
        if name_after:
            print(f"  Page {page_num}, after '{name_after}', {len(all_projects)} projects so far...")
        else:
            print(f"  Page {page_num} (first page)...")

        data = fetch_page(name_after)

        if not data:
            print("  Empty response, pagination complete.")
            break

        all_projects.update(data)

        # Repology returns up to 200 projects per page.
        # If we get fewer, we're done.
        project_names = sorted(data.keys())
        if len(project_names) < 200:
            print(f"  Got {len(project_names)} projects (< 200), pagination complete.")
            break

        name_after = project_names[-1]
        time.sleep(REQUEST_DELAY)

    print(f"Total projects fetched: {len(all_projects)}")
    return all_projects


def build_distro_map(all_projects, distro_key, distro_repo):
    """Build the mapping dict for a single distro."""
    print(f"Building mappings for {distro_key} ({distro_repo})...")
    mappings = extract_mappings(all_projects, distro_repo)
    print(f"  {len(mappings)} mappings found.")
    return mappings


def write_map_file(filepath, distro_repo, mappings):
    """Write a single mapping JSON file."""
    now = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    doc = {
        "metadata": {
            "generated_at": now,
            "source": "repology",
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
            "source": "repology",
            "distros": list(DISTRO_REPOS.values()),
            "total_mappings": total,
        },
        "distros": combined,
    }
    os.makedirs(os.path.dirname(filepath), exist_ok=True)
    with open(filepath, "w") as f:
        json.dump(doc, f, indent=2)
    size_kb = os.path.getsize(filepath) / 1024
    print(f"  Wrote {filepath} ({size_kb:.1f} KB, {total} total mappings across all distros)")


def main():
    print("=" * 60)
    print("RepoSources - Package Map Generator")
    print("=" * 60)

    # Fetch all project data from Repology
    all_projects = paginate_all()

    if not all_projects:
        print("ERROR: No project data retrieved from Repology. Exiting.")
        sys.exit(1)

    # Build per-distro mappings
    distro_maps = {}
    for distro_key, distro_repo in DISTRO_REPOS.items():
        distro_maps[distro_key] = build_distro_map(all_projects, distro_key, distro_repo)

    # Write individual distro files
    print("\nWriting output files...")
    for distro_key, mappings in distro_maps.items():
        filepath = os.path.join(OUTPUT_DIR, f"{distro_key}.json")
        write_map_file(filepath, DISTRO_REPOS[distro_key], mappings)

    # Write combined file
    all_filepath = os.path.join(OUTPUT_DIR, "all.json")
    write_all_file(all_filepath, distro_maps)

    print("\nDone!")


if __name__ == "__main__":
    main()
