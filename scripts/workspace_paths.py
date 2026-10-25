from pathlib import Path


def resolve_within(path: Path, roots: tuple[Path, ...]) -> Path:
    resolved_roots = tuple(root.resolve() for root in roots)
    candidate = path.expanduser()
    if not candidate.is_absolute():
        candidate = resolved_roots[0] / candidate
    resolved = candidate.resolve()
    if not any(resolved.is_relative_to(root) for root in resolved_roots):
        raise ValueError(f"path escapes its allowed directory: {path}")
    return resolved
