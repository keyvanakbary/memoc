#!/usr/bin/env python3
"""Plan a main-branch release: breaking > feat > patch (all other commits).

Apply one bump for all commits since the last stable tag. Before the first tag,
use Cargo.toml as the baseline. A rerun at the release tag resumes that version.
"""
import os
from pathlib import Path
import re
import subprocess
import tomllib


STABLE = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)")


def parse_version(value):
    match = STABLE.fullmatch(value)
    if not match:
        raise ValueError(f"Expected a stable MAJOR.MINOR.PATCH version, got {value!r}")
    return tuple(map(int, match.groups()))


def bump(version, messages):
    level = 0
    for message in messages:
        subject = message.splitlines()[0] if message else ""
        if re.match(r"^[a-z]+(?:\([^\n)]+\))?!:", subject) or re.search(
            r"^BREAKING[ -]CHANGE:\s*\S", message, re.MULTILINE
        ):
            level = 2
        elif re.match(r"^feat(?:\([^\n)]+\))?:", subject):
            level = max(level, 1)
    major, minor, patch = parse_version(version)
    return ".".join(map(str, (major + 1, 0, 0) if level == 2 else
                        (major, minor + 1, 0) if level == 1 else (major, minor, patch + 1)))


def git(root, *args):
    return subprocess.check_output(["git", *args], cwd=root, text=True).strip()


def prepare(root):
    manifest_path, lock_path = root / "Cargo.toml", root / "Cargo.lock"
    manifest, lock = manifest_path.read_text(), lock_path.read_text()
    package = tomllib.loads(manifest)["package"]
    current = package["version"]
    parse_version(current)
    locked = [p for p in tomllib.loads(lock)["package"] if p["name"] == package["name"] and "source" not in p]
    if len(locked) != 1 or locked[0]["version"] != current:
        raise ValueError("Cargo.toml and Cargo.lock versions must match")

    tags = [tag for tag in git(root, "tag", "--list", "v*").splitlines() if STABLE.fullmatch(tag[1:])]
    previous = max(tags, key=lambda tag: parse_version(tag[1:])) if tags else None
    if previous:
        subprocess.run(["git", "merge-base", "--is-ancestor", previous, "HEAD"], cwd=root, check=True)
        if current != previous[1:]:
            raise ValueError(f"Cargo version must match {previous}; CI handles version bumps automatically")
        if git(root, "rev-parse", "HEAD") == git(root, "rev-parse", f"{previous}^{{commit}}"):
            return {"version": current, "tag": previous, "changed": "false"}

    messages = git(root, "log", "--format=%B%x00", f"{previous}..HEAD" if previous else "HEAD").split("\0")
    version = bump(current, [message.strip() for message in messages if message.strip()])
    manifest, count = re.subn(
        r'(?m)(^version\s*=\s*")' + re.escape(current) + r'("\s*$)',
        lambda match: match[1] + version + match[2], manifest, count=1,
    )
    if count != 1 or tomllib.loads(manifest)["package"]["version"] != version:
        raise ValueError("Could not update the package version in Cargo.toml")
    lock, count = re.subn(
        r'(?m)(^name = "' + re.escape(package["name"]) + r'"\nversion = ")' + re.escape(current) + r'(")',
        lambda match: match[1] + version + match[2], lock,
    )
    if count != 1:
        raise ValueError("Could not uniquely update the package version in Cargo.lock")
    manifest_path.write_text(manifest)
    lock_path.write_text(lock)
    return {"version": version, "tag": f"v{version}", "changed": "true"}


if __name__ == "__main__":
    plan = prepare(Path.cwd())
    if output := os.environ.get("GITHUB_OUTPUT"):
        with open(output, "a") as stream:
            stream.write("".join(f"{key}={value}\n" for key, value in plan.items()))
    print(f"Release {plan['tag']} ({'new version' if plan['changed'] == 'true' else 'resume'})")
