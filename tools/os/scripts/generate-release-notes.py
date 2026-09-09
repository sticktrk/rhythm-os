#!/usr/bin/env python3
"""Generate channel-aware notes from published releases and local public Git history."""

import argparse
import json
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[3]
TAG = re.compile(r"v(\d+)\.(\d+)\.(\d+)-(beta|stable)")
VERSION_BUMP = re.compile(r"Release v\d+\.\d+\.\d+-(beta|stable)")


def git(*args, check=True):
    return subprocess.run(["git", "-C", str(ROOT), *args], check=check,
                          text=True, capture_output=True)


def version(tag):
    match = TAG.fullmatch(tag)
    if not match:
        return None
    return (*map(int, match.groups()[:3]), match[4] == "stable")


def read_catalog(path):
    releases = json.loads(path.read_text())
    if not isinstance(releases, list) or not all(isinstance(r, dict) for r in releases):
        raise ValueError(f"Expected a JSON array of release metadata in {path}")
    return releases


def published_releases(repository):
    # --slurp preserves page boundaries; flatten after successful pagination.
    result = subprocess.run(
        ["gh", "api", "--paginate", "--slurp", f"repos/{repository}/releases?per_page=100"],
        check=True, text=True, capture_output=True)
    return [release for page in json.loads(result.stdout) for release in page]


def previous_tag(current_tag, current_commit, releases):
    current_version = version(current_tag)
    # When regenerating an existing release, don't introduce later publications.
    published_at = next((r.get("published_at") for r in releases
                         if r.get("tag_name") == current_tag and not r.get("draft")), None)
    candidates = set()
    for release in releases:
        tag = release.get("tag_name", "")
        candidate_version = version(tag)
        if release.get("draft") is not False or not release.get("published_at"):
            continue
        if not candidate_version or candidate_version >= current_version:
            continue
        if current_tag.endswith("-stable") and not tag.endswith("-stable"):
            continue
        if published_at and release["published_at"] > published_at:
            continue
        candidates.add(tag)

    # For a beta, the newest stable supersedes earlier betas. Stable notes
    # remain cumulative across every intervening beta in that release cycle.
    for tag in sorted(candidates, key=version, reverse=True):
        result = git("rev-parse", "--verify", f"refs/tags/{tag}^{{commit}}", check=False)
        if result.returncode:
            raise ValueError(f"Published release tag {tag} is missing locally; fetch full history and tags")
        ancestor = git("merge-base", "--is-ancestor", result.stdout.strip(), current_commit, check=False)
        if ancestor.returncode == 0:
            return tag
        if ancestor.returncode != 1:
            ancestor.check_returncode()
    return None


def generate(current_tag, repository, releases):
    current_commit = git("rev-parse", "--verify", f"refs/tags/{current_tag}^{{commit}}").stdout.strip()
    previous = previous_tag(current_tag, current_commit, releases)
    revision = f"refs/tags/{previous}..{current_commit}" if previous else current_commit
    subjects = git("log", "--no-merges", "--format=%s", revision).stdout.splitlines()
    changes = [f"- {subject}" for subject in subjects if not VERSION_BUMP.fullmatch(subject)]
    url = f"https://github.com/{repository}"
    lines = [f"# Rhythm {current_tag}", "", f"Public source: {url}/tree/{current_commit}", ""]
    if previous:
        lines += [f"Changes since {previous}:", ""]
    else:
        lines += [f"Changes in this first published {current_tag.rsplit('-', 1)[1]} release:", ""]
    lines += changes or ["No additional source changes."]
    if previous:
        lines += ["", f"[Full changelog]({url}/compare/{previous}...{current_tag})"]
    return "\n".join(lines) + "\n", previous


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("tag", help="Current vX.Y.Z-beta or vX.Y.Z-stable tag")
    parser.add_argument("output", type=Path)
    parser.add_argument("--repository", help="Public owner/repo; defaults to the checkout's GitHub repository")
    parser.add_argument("--published-releases", type=Path,
                        help="Use a JSON release catalog instead of querying GitHub (offline preview)")
    parser.add_argument("--additional-publications", type=Path, action="append", default=[],
                        help="Supplement with published tag metadata from a previous publisher")
    args = parser.parse_args()
    if version(args.tag) is None:
        parser.error("tag must be vX.Y.Z-beta or vX.Y.Z-stable")
    repository = args.repository
    if not repository:
        repository = subprocess.check_output(
            ["gh", "repo", "view", "--json", "nameWithOwner", "--jq", ".nameWithOwner"],
            cwd=ROOT, text=True).strip()
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository):
        parser.error("repository must be owner/repo")
    releases = (read_catalog(args.published_releases) if args.published_releases
                else published_releases(repository))
    for path in args.additional_publications:
        releases.extend(read_catalog(path))
    body, previous = generate(args.tag, repository, releases)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(body)
    print(f"Release notes: {args.tag} (previous: {previous or 'none'}) -> {args.output}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"Error: {error}", file=sys.stderr)
        sys.exit(1)
