import argparse
import re
import sys
from pathlib import Path

VERSION = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)")
VERSION_LINE = re.compile(r'^\s*version "([^"]*)"\s*$', re.MULTILINE)
CLASS_LINE = "class Agentdust < Formula"
FORMULA_DIR = "Formula"


class TapError(Exception):
    pass


def parse(version):
    match = VERSION.fullmatch(version)
    if not match:
        raise TapError(f'"{version}" is not a version of the form MAJOR.MINOR.PATCH')
    return tuple(int(part) for part in match.groups())


def class_name(key):
    return "AgentdustAT" + key.replace(".", "")


def formula_version(text, source):
    match = VERSION_LINE.search(text)
    if not match:
        raise TapError(f"{source} has no version line")
    parse(match.group(1))
    return match.group(1)


def require_class(text, source):
    if CLASS_LINE not in text.splitlines():
        raise TapError(f'{source} does not declare "{CLASS_LINE}"')


def read(path):
    try:
        return path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as error:
        raise TapError(f"cannot read {path}: {error}") from error


def planned_writes(tap, text, version, source):
    new = parse(version)
    if formula_version(text, source) != version:
        raise TapError(f"{source} holds another version than {version}")
    require_class(text, source)
    current = tap / FORMULA_DIR / "agentdust.rb"
    writes = []
    if current.exists():
        old_text = read(current)
        old_version = formula_version(old_text, current)
        old = parse(old_version)
        if new < old:
            raise TapError(f"version {version} is older than {old_version}, which the tap already holds")
        if new == old and old_text != text:
            raise TapError(f"the tap already holds version {version} with another content")
        if new[:2] != old[:2]:
            require_class(old_text, current)
            key = f"{old[0]}.{old[1]}"
            versioned = tap / FORMULA_DIR / f"agentdust@{key}.rb"
            content = old_text.replace(CLASS_LINE, f"class {class_name(key)} < Formula", 1)
            if versioned.exists() and read(versioned) != content:
                raise TapError(f"{versioned.name} exists with other content and is not overwritten")
            writes.append((versioned, content))
    writes.append((current, text))
    return writes


def update_tap(tap_dir, formula, version):
    tap = Path(tap_dir)
    source = Path(formula)
    writes = planned_writes(tap, read(source), version, source)
    for path, content in writes:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")
    return [str(path.relative_to(tap)) for path, _ in writes]


def main(argv=None):
    parser = argparse.ArgumentParser(description="Put a release formula into a checkout of the tap.")
    parser.add_argument("--tap", type=Path, required=True)
    parser.add_argument("--formula", type=Path, required=True)
    parser.add_argument("--version", required=True)
    args = parser.parse_args(argv)
    try:
        written = update_tap(args.tap, args.formula, args.version)
    except (TapError, OSError) as error:
        print(f"tap update failed: {error}", file=sys.stderr)
        return 1
    for path in written:
        print(path)
    return 0


if __name__ == "__main__":
    sys.exit(main())
