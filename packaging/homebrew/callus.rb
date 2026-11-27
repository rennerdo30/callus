class Callus < Formula
  desc "Local-first mistake memory, PreToolUse guard, and rule compiler for AI coding CLIs"
  homepage "https://github.com/rennerdo30/callus"
  license "MIT"
  version "0.1.0"

  on_macos do
    on_arm do
      url "https://github.com/rennerdo30/callus/releases/download/v#{version}/callus-v#{version}-aarch64-apple-darwin.tar.gz"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/rennerdo30/callus/releases/download/v#{version}/callus-v#{version}-x86_64-unknown-linux-gnu.tar.gz"
    end
  end

  def install
    bin.install "callus"
    pkgshare.install "packs"
  end

  test do
    assert_match "callus", shell_output("#{bin}/callus --version")
  end
end
