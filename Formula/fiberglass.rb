class Fiberglass < Formula
  desc "Fiberglass — a trading terminal for Polymarket"
  homepage "https://github.com/jesodium/fiberglass"
  version "0.1.17"
  license "MIT"

  on_macos do
    on_intel do
      url "https://github.com/jesodium/fiberglass/releases/download/v#{version}/fiberglass-v#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "283941a4bedfa18d2e3e8de2de09c4b35c59f80fa28900cc5f326aea448e10b9"
    end

    on_arm do
      url "https://github.com/jesodium/fiberglass/releases/download/v#{version}/fiberglass-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "b52688d8a8d178603870912322bd62059e918df4c9d8292946ace9c0d83cbeee"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/jesodium/fiberglass/releases/download/v#{version}/fiberglass-v#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "c58eba077d31be642adc94edc2b22be628f50ac84a7e1d0116515ed785e33d81"
    end

    on_arm do
      url "https://github.com/jesodium/fiberglass/releases/download/v#{version}/fiberglass-v#{version}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "7b8ac252f147a1f88c40593207c239fe403eb056f15608018183bb9b5e4e33c6"
    end
  end

  def install
    bin.install "fiberglass"
  end

  test do
    assert_match "fiberglass", shell_output("#{bin}/fiberglass --version")
  end
end
