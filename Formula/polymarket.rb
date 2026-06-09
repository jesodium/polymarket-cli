class Polymarket < Formula
  desc "CLI for Polymarket — browse markets, trade, and manage positions"
  homepage "https://github.com/jesodium/polymarket-cli"
  version "0.1.5"
  license "MIT"

  on_macos do
    on_intel do
      url "https://github.com/jesodium/polymarket-cli/releases/download/v#{version}/polymarket-v#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "5efde783327046ee0fba33baf4f0b9e9e7cbac75ba7482bac57f676604581f31"
    end

    on_arm do
      url "https://github.com/jesodium/polymarket-cli/releases/download/v#{version}/polymarket-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "1e0c57c0a68baa6f7e3469d684b770263048923bfc673ec7e7061db5156427bf"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/jesodium/polymarket-cli/releases/download/v#{version}/polymarket-v#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "dd9c8c9f396588427efd7f538474b2a11434c21fdf6ca90aa75a0edb2bd3d8a6"
    end

    on_arm do
      url "https://github.com/jesodium/polymarket-cli/releases/download/v#{version}/polymarket-v#{version}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "1a33c92c6ffb21172254496d400e4a8bf6996cd00ee90e05199be47600057cda"
    end
  end

  def install
    bin.install "polymarket"
  end

  test do
    assert_match "polymarket", shell_output("#{bin}/polymarket --version")
  end
end
