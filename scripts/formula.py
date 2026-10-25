import argparse
import hashlib
from pathlib import Path

from workspace_paths import resolve_within

TEMPLATE = """class Agentdust < Formula
  desc "Finds and cleans processes that AI coding agents leave behind"
  homepage "https://github.com/hamzahamidi/agentdust"
  url "{url}"
  version "{version}"
  sha256 "{sha256}"

  depends_on arch: :arm64
  depends_on :macos

  def install
    bin.install "agentdust"
  end

  test do
    assert_match "agentdust #{{version}}", shell_output("#{{bin}}/agentdust version")
  end
end
"""


def main() -> None:
    parser = argparse.ArgumentParser(description="Write the Homebrew formula for one agentdust tarball.")
    parser.add_argument("--tarball", required=True, type=Path)
    parser.add_argument("--version", required=True)
    parser.add_argument("--url", required=True)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()

    root = Path.cwd()
    try:
        tarball = resolve_within(args.tarball, (root,))
        output = resolve_within(args.out, (root,))
    except ValueError as error:
        parser.error(str(error))
    sha256 = hashlib.sha256(tarball.read_bytes()).hexdigest()
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(TEMPLATE.format(url=args.url, version=args.version, sha256=sha256))
    print(output)


if __name__ == "__main__":
    main()
