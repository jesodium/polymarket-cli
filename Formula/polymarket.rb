class Polymarket < Formula
  desc "CLI for Polymarket — browse markets, trade, and manage positions"
  homepage "https://github.com/jesodium/polymarket-cli"
  version "0.1.11"
  license "MIT"

  on_macos do
    on_intel do
      url "https://github.com/jesodium/polymarket-cli/releases/download/v#{version}/polymarket-v#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "4f6a7b0c1e2cea295748543004a82f9a46e8ea5aff11b82b22d0f6a1ca59ac47"
    end

    on_arm do
      url "https://github.com/jesodium/polymarket-cli/releases/download/v#{version}/polymarket-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "49077741a1223337c94ab8dbf3e2620d0acc48bd55ba7fd8cd6bb0de7cf6ea6e"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/jesodium/polymarket-cli/releases/download/v#{version}/polymarket-v#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "ef88e2ae5ed54b68ddaa3df9ca9086248c2dc8a569360ce851a8d4af12d7af44"
    end

    on_arm do
      url "https://github.com/jesodium/polymarket-cli/releases/download/v#{version}/polymarket-v#{version}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "afd361dba2e2338df86571e446b136da66c14550f05b3846eeed2fbec9548349"
    end
  end

  def install
    bin.install "polymarket"
  end

  test do
    assert_match "polymarket", shell_output("#{bin}/polymarket --version")
  end
end
