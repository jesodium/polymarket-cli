# fiberglass

a rewritten fork of the polymarket cli built for AI agent support and backtesting with paper trading.

[![macos ci](https://github.com/jesodium/fiberglass/actions/workflows/ci-macos.yml/badge.svg?branch=main)](https://github.com/jesodium/fiberglass/actions/workflows/ci-macos.yml?query=branch%3Amain)

[![linux ci](https://github.com/jesodium/fiberglass/actions/workflows/ci-linux.yml/badge.svg?branch=main)](https://github.com/jesodium/fiberglass/actions/workflows/ci-linux.yml?query=branch%3Amain)

![windows ci](https://img.shields.io/badge/windows%20ci-disabled-red)

> **windows support:** CI and release builds for windows are temporarily disabled, the windows-latest GitHub Actions runner hangs indefinitely partway through the test suite (not a crash, not just slow; still unresolved). This is a CI-environment issue, not a known problem with the app itself `cargo install --path .` should still work if you're building from source on windows. No windows binary is published for this release and a fix date is not planned.
>
## Use your own money at your own risk!
The live mode has not been tested yet; if you lose your money, it's your problem.

## install

```bash
# homebrew
brew tap jesodium/fiberglass https://github.com/jesodium/fiberglass
brew install fiberglass

# or the install script
curl -sSL https://raw.githubusercontent.com/jesodium/fiberglass/main/install.sh | sh

# or from source
git clone https://github.com/jesodium/fiberglass
cd fiberglass
cargo install --path .
```

## the TUI

main way to use it:

```bash
fiberglass tui            # live mode, needs a wallet
fiberglass tui --paper    # paper mode, $10k fake money, no wallet
```

## config

lives in `~/.config/polymarket/` your wallet, settings, paper account, guards,
copytrade roster. your private key goes in the OS keychain (macOS Keychain,
Windows Credential Manager, Linux Secret Service). on a headless box with no
keychain it falls back to a plaintext file (`0600`, owner-only) — keep that
machine locked down, or pass the key via `POLYMARKET_PRIVATE_KEY` / `--key`
to keep it off disk entirely.

see [changelog.md](CHANGELOG.md) for what changed.

## stars

[![star history](https://api.star-history.com/svg?repos=jesodium/fiberglass&type=Date)](https://star-history.com/#jesodium/fiberglass&Date)

## license

GPL-3.0-or-later. See [LICENSE](LICENSE). You may use, modify, and redistribute,
but derivatives must stay open under the same license. No warranty — see §15–17.

# ai usage
ai was used in the making of this project (claude code)
