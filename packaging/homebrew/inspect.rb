# Homebrew formula template for `inspect`.
#
# Lives in this repo as a reference. Publish to a tap by:
#   1. Create repo `homebrew-tap` under your user/org.
#   2. After tagging v0.1.0, copy this file to `homebrew-tap/Formula/inspect.rb`
#      and replace the four `__SHA256__*__` placeholders with the values from
#      the release artifacts (`<artifact>.tar.gz.sha256`).
#   3. `brew tap jpbeaudet/tap && brew install inspect`.
#
# Do NOT publish to homebrew/core for v0.1.0 — homebrew/core requires
# notable usage and a stable release cadence.

class Inspect < Formula
  desc "Operational debugging CLI for cross-server search and safe hot-fix application"
  homepage "https://github.com/jpbeaudet/inspect"
  version "0.1.0"
  license "Apache-2.0"

  on_macos do
    on_arm do
      url "https://github.com/jpbeaudet/inspect/releases/download/v#{version}/inspect-#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "__SHA256_DARWIN_ARM64__"
    end
    on_intel do
      url "https://github.com/jpbeaudet/inspect/releases/download/v#{version}/inspect-#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "__SHA256_DARWIN_X86_64__"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/jpbeaudet/inspect/releases/download/v#{version}/inspect-#{version}-aarch64-unknown-linux-musl.tar.gz"
      sha256 "__SHA256_LINUX_ARM64__"
    end
    on_intel do
      url "https://github.com/jpbeaudet/inspect/releases/download/v#{version}/inspect-#{version}-x86_64-unknown-linux-musl.tar.gz"
      sha256 "__SHA256_LINUX_X86_64__"
    end
  end

  depends_on "openssh"

  def install
    bin.install "inspect"
  end

  test do
    assert_match "inspect #{version}", shell_output("#{bin}/inspect --version")
  end
end
