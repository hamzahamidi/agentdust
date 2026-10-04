import argparse
import re
import sys
from pathlib import Path

ADVERSARIES = (
    "Malicious model",
    "Malicious process metadata",
    "Buggy MCP client",
    "Same-user tampering",
    "PID reuse",
    "Concurrent apply",
    "Stale plans",
    "Compromised release artifact",
    "Unsupported filesystem",
    "Journal poisoning and short writes",
    "Stale writers and rotation",
    "Durability limits",
)
DEFAULT_MODEL = "docs/threat-model.md"
DEFAULT_SPEC = "docs/superpowers/specs/2026-10-03-agentdust-design.md"

FENCE = re.compile(r"^\s*(```|~~~)")
HEADING = re.compile(r"^##\s+(?:\d+\.\s+)?(.+?)\s*$")
SECTION_91 = re.compile(r"^#{2,3}\s+9\.1\b")
ANY_HEADING = re.compile(r"^#{1,3}\s")
SPEC_ROW = re.compile(r"^\|\s*(S\d+)\s*\|")
CITATION = re.compile(r"\bS\d+\b")
SEPARATOR_CELL = re.compile(r"^:?-{3,}:?$")
STATUS = re.compile(r"implemented|planned m\d")
REQUIREMENT = re.compile(r"(?i:none)|S\d+(?:(?:\s*,\s*|\s+)S\d+)*")
RESIDUAL = re.compile(r"^\*\*Residual risk\.\*\*")
BULLET = re.compile(r"^\s*[-*]\s+\S")
PATH_SPAN = re.compile(r"`([^`\s]*/[^`\s]*)`")


def prose_lines(text):
    fenced = False
    for number, line in enumerate(text.splitlines(), 1):
        if FENCE.match(line):
            fenced = not fenced
            continue
        if not fenced:
            yield number, line


def spec_requirements(spec_text):
    ids = []
    inside = False
    for _, line in prose_lines(spec_text):
        if SECTION_91.match(line):
            inside = True
        elif inside and ANY_HEADING.match(line):
            break
        elif inside:
            row = SPEC_ROW.match(line)
            if row:
                ids.append(row.group(1))
    return ids


def sections(model_text):
    found = {}
    current = None
    for _, line in prose_lines(model_text):
        heading = HEADING.match(line)
        if heading:
            current = found.setdefault(heading.group(1).lower(), [])
        elif current is not None:
            current.append(line)
    return found


def cells(line):
    return [cell.strip() for cell in re.split(r"(?<!\\)\|", line.strip().strip("|"))]


def control_rows(model_text):
    header = None
    columns = None
    for number, line in prose_lines(model_text):
        if not line.lstrip().startswith("|"):
            header = columns = None
            continue
        row = cells(line)
        if header is None:
            header = row
        elif all(SEPARATOR_CELL.match(cell) for cell in row):
            columns = {}
            for index, cell in enumerate(header):
                columns.setdefault(cell.lower(), index)
            if "status" not in columns:
                columns = None
        elif columns is not None:
            yield number, row, columns


def in_repository(root, token):
    base = root.resolve()
    target = (base / token).resolve()
    return target.is_relative_to(base) and target.exists()


def check_headings(model_text):
    present = sections(model_text)
    return [
        f'missing heading "## {title}"'
        for title in ADVERSARIES
        if title.lower() not in present
    ]


def check_residual(model_text):
    present = sections(model_text)
    problems = []
    for title in ADVERSARIES:
        lines = present.get(title.lower())
        if lines is None:
            continue
        marker = next((i for i, line in enumerate(lines) if RESIDUAL.match(line)), None)
        if marker is None:
            problems.append(f'section "{title}" has no "Residual risk." part')
        elif not any(BULLET.match(line) for line in lines[marker + 1 :]):
            problems.append(f'section "{title}" lists nothing under "Residual risk."')
    return problems


def check_citations(model_text, known):
    problems = []
    cited = set()
    for number, line in prose_lines(model_text):
        for requirement in dict.fromkeys(CITATION.findall(line)):
            cited.add(requirement)
            if known and requirement not in known:
                problems.append(f"line {number}: {requirement} is not a requirement in spec section 9.1")
    problems += [
        f"{requirement} of spec section 9.1 is not cited anywhere"
        for requirement in sorted(known - cited, key=lambda item: int(item[1:]))
    ]
    return problems


def check_rows(model_text, root):
    problems = []
    for number, row, columns in control_rows(model_text):
        if "requirement" in columns:
            requirement = row[columns["requirement"]] if columns["requirement"] < len(row) else ""
            if not REQUIREMENT.fullmatch(requirement):
                problems.append(
                    f'line {number}: requirement "{requirement}" is not "none" or a list of S-ids'
                )
        column = columns["status"]
        status = row[column].lower() if column < len(row) else ""
        if not STATUS.fullmatch(status):
            problems.append(
                f'line {number}: status "{status}" is not "implemented" or "planned M<n>"'
            )
        elif status == "implemented":
            paths = [token for cell in row for token in PATH_SPAN.findall(cell)]
            if not paths:
                problems.append(f"line {number}: an implemented row names no repository path")
            problems += [
                f"line {number}: {token} is not a file in the repository"
                for token in paths
                if not in_repository(root, token)
            ]
    return problems


def check(model_text, spec_text, root):
    known = set(spec_requirements(spec_text))
    problems = []
    if not known:
        problems.append("spec section 9.1 holds no requirement ids")
    problems += check_headings(model_text)
    problems += check_residual(model_text)
    problems += check_rows(model_text, root)
    problems += check_citations(model_text, known)
    return problems


def read(path):
    try:
        return path.read_text(encoding="utf-8"), None
    except (OSError, UnicodeDecodeError) as error:
        return None, f"cannot read {path}: {error}"


def main(argv=None):
    parser = argparse.ArgumentParser(description="Check the threat model against the spec and the repository.")
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parent.parent)
    parser.add_argument("--model", type=Path, default=Path(DEFAULT_MODEL))
    parser.add_argument("--spec", type=Path, default=Path(DEFAULT_SPEC))
    args = parser.parse_args(argv)

    model_path = args.root / args.model
    model_text, model_error = read(model_path)
    spec_text, spec_error = read(args.root / args.spec)
    problems = [error for error in (model_error, spec_error) if error]
    if model_text is not None and spec_text is not None:
        problems = [f"{model_path}: {problem}" for problem in check(model_text, spec_text, args.root)]
    for problem in problems:
        print(problem, file=sys.stderr)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
