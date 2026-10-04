import argparse
import json
import re
import subprocess
import sys
import tomllib
from pathlib import Path

TAG = re.compile(r"v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)")
OBJECT_ID = re.compile(r"[0-9a-f]{40}|[0-9a-f]{64}")
REQUIRED_CHECKS = ("linux", "macos")
ACTIONS_APP = "github-actions"
CI_HINT = "run the ci workflow on this commit (gh workflow run ci.yml --ref main), wait for it to pass, then release again"


class ReleaseError(Exception):
    pass


def tag_version(tag):
    if not TAG.fullmatch(tag):
        raise ReleaseError(f'"{tag}" is not a release tag of the form vMAJOR.MINOR.PATCH')
    return tag[1:]


def table(value, key):
    child = value.get(key) if isinstance(value, dict) else None
    return child if isinstance(child, dict) else {}


def crate_version(text):
    try:
        document = tomllib.loads(text)
    except tomllib.TOMLDecodeError as error:
        raise ReleaseError(f"Cargo.toml is not valid TOML: {error}") from error
    version = table(table(document, "workspace"), "package").get("version")
    if not isinstance(version, str):
        raise ReleaseError("Cargo.toml has no version under [workspace.package]")
    return version


def check_tag(tag, cargo_toml_text):
    version = tag_version(tag)
    crate = crate_version(cargo_toml_text)
    if version != crate:
        raise ReleaseError(f"tag {tag} names version {version} but the crate version is {crate}")
    return version


def git(repo, *args):
    return subprocess.run(["git", *args], cwd=repo, capture_output=True, text=True)


def tag_commit(tag, repo="."):
    tag_version(tag)
    result = git(repo, "rev-parse", "--verify", "--quiet", f"refs/tags/{tag}^{{commit}}")
    if result.returncode != 0:
        raise ReleaseError(f"tag {tag} does not exist in this repository")
    return result.stdout.strip()


def check_reachable(commit, main_ref, repo="."):
    if not OBJECT_ID.fullmatch(commit):
        raise ReleaseError(f'"{commit}" is not a full commit id')
    if main_ref.startswith("-"):
        raise ReleaseError(f'"{main_ref}" is not a branch name')
    shallow = git(repo, "rev-parse", "--is-shallow-repository")
    if shallow.stdout.strip() == "true":
        raise ReleaseError("the checkout is shallow and cannot show whether main contains the commit; fetch the full history")
    if git(repo, "rev-parse", "--verify", "--quiet", f"{main_ref}^{{commit}}").returncode != 0:
        raise ReleaseError(f"{main_ref} does not exist in this repository")
    result = git(repo, "merge-base", "--is-ancestor", commit, main_ref)
    if result.returncode == 1:
        raise ReleaseError(f"commit {commit} is not reachable from {main_ref}")
    if result.returncode != 0:
        raise ReleaseError(f"git could not compare {commit} with {main_ref}: {result.stderr.strip()}")


def listing_pages(text):
    decoder = json.JSONDecoder()
    pages = []
    index = 0
    while True:
        while index < len(text) and text[index].isspace():
            index += 1
        if index == len(text):
            break
        try:
            value, index = decoder.raw_decode(text, index)
        except json.JSONDecodeError as error:
            raise ReleaseError(f"the check run listing is not JSON: {error}") from error
        pages.extend(value if isinstance(value, list) else [value])
    if not pages:
        raise ReleaseError("the check run listing is empty")
    return pages


def listed_runs(text):
    runs = []
    for page in listing_pages(text):
        listed = page.get("check_runs") if isinstance(page, dict) else None
        if not isinstance(listed, list) or not all(isinstance(run, dict) for run in listed):
            raise ReleaseError("the check run listing has no check_runs list of objects")
        runs.extend(listed)
    return runs


def newest_runs(runs, commit):
    newest = {}
    for run in runs:
        app = run.get("app")
        name = run.get("name")
        run_id = run.get("id")
        if run.get("head_sha") != commit or not isinstance(app, dict) or app.get("slug") != ACTIONS_APP:
            continue
        if isinstance(name, str) and isinstance(run_id, int) and run_id > newest.get(name, {}).get("id", -1):
            newest[name] = run
    return newest


def check_run_problems(text, commit, required=REQUIRED_CHECKS):
    newest = newest_runs(listed_runs(text), commit)
    problems = []
    for name in required:
        run = newest.get(name)
        if run is None:
            problems.append(f'no GitHub Actions check run named "{name}" for commit {commit}')
        elif run.get("status") != "completed" or run.get("conclusion") != "success":
            problems.append(
                f'check "{name}" is {run.get("status")} with conclusion {run.get("conclusion")}, not completed with success'
            )
    return problems


def read(path):
    try:
        return path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as error:
        raise ReleaseError(f"cannot read {path}: {error}") from error


def run_tag(args):
    print(check_tag(args.tag, read(args.cargo_toml)))


def run_reachable(args):
    commit = tag_commit(args.tag, args.repo)
    check_reachable(commit, args.main_ref, args.repo)
    print(commit)


def run_checks(args):
    required = tuple(args.require) if args.require else REQUIRED_CHECKS
    problems = check_run_problems(read(args.runs), args.commit, required)
    if problems:
        raise ReleaseError("\n".join([*problems, CI_HINT]))
    for name in required:
        print(f"{name}: success")


def parser():
    root = argparse.ArgumentParser(description="Gate a release tag before anything is built.")
    commands = root.add_subparsers(dest="command", required=True)
    tag = commands.add_parser("tag", help="the tag is vX.Y.Z and equals the crate version; prints the version")
    tag.add_argument("--tag", required=True)
    tag.add_argument("--cargo-toml", type=Path, default=Path("Cargo.toml"))
    tag.set_defaults(run=run_tag)
    reachable = commands.add_parser("reachable", help="the commit of the tag is reachable from main; prints the commit")
    reachable.add_argument("--tag", required=True)
    reachable.add_argument("--main-ref", required=True)
    reachable.add_argument("--repo", default=".")
    reachable.set_defaults(run=run_reachable)
    checks = commands.add_parser("checks", help="the required check runs passed on the commit")
    checks.add_argument("--commit", required=True)
    checks.add_argument("--runs", type=Path, required=True)
    checks.add_argument("--require", action="append")
    checks.set_defaults(run=run_checks)
    return root


def main(argv=None):
    args = parser().parse_args(argv)
    try:
        args.run(args)
    except ReleaseError as error:
        print(f"release check failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
