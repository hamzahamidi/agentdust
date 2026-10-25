import argparse
import gzip
import hashlib
import io
import os
import tarfile
import tempfile
from pathlib import Path

from workspace_paths import resolve_within


def add_file(tar: tarfile.TarFile, path: Path, arcname: str, mode: int, mtime: int) -> None:
    data = path.read_bytes()
    info = tarfile.TarInfo(arcname)
    info.size = len(data)
    info.mode = mode
    info.mtime = mtime
    info.uid = info.gid = 0
    info.uname = info.gname = ""
    tar.addfile(info, io.BytesIO(data))


def main() -> None:
    parser = argparse.ArgumentParser(description="Create a reproducible AgentDust release tarball.")
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--version", required=True)
    parser.add_argument("--target", default="darwin-arm64")
    parser.add_argument("--out-dir", required=True, type=Path)
    args = parser.parse_args()

    root = Path.cwd()
    try:
        binary = resolve_within(args.binary, (root,))
        out_dir = resolve_within(args.out_dir, (root, Path(tempfile.gettempdir())))
    except ValueError as error:
        parser.error(str(error))

    mtime = int(os.environ.get("SOURCE_DATE_EPOCH", "0"))
    name = f"agentdust-{args.version}-{args.target}"
    files = [
        (binary, f"{name}/agentdust", 0o755),
        (Path("LICENSE"), f"{name}/LICENSE", 0o644),
        (Path("README.md"), f"{name}/README.md", 0o644),
    ]
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w", format=tarfile.USTAR_FORMAT) as tar:
        for path, arcname, mode in sorted(files, key=lambda entry: entry[1]):
            add_file(tar, path, arcname, mode, mtime)

    out_dir.mkdir(parents=True, exist_ok=True)
    out = out_dir / f"{name}.tar.gz"
    with out.open("wb") as handle, gzip.GzipFile(
        filename="", mode="wb", fileobj=handle, mtime=0, compresslevel=9
    ) as gz:
        gz.write(raw.getvalue())
    print(f"{hashlib.sha256(out.read_bytes()).hexdigest()}  {out.name}")


if __name__ == "__main__":
    main()
