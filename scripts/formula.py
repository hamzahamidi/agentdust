import argparse
import hashlib
from pathlib import Path

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

    sha256 = hashlib.sha256(args.tarball.read_bytes()).hexdigest()
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(TEMPLATE.format(url=args.url, version=args.version, sha256=sha256))
    print(args.out)


if __name__ == "__main__":
    main()
