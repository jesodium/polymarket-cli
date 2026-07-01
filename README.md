# fiberglass

a rewritten fork of the polymarket cli built for AI agent support and backtesting with paper trading.

![macos](https://img.shields.io/badge/macos-000000?logo=apple&logoColor=white)
![linux](https://img.shields.io/badge/linux-FCC624?logo=linux&logoColor=black)
![windows](https://img.shields.io/badge/windows-0078D6?logo=windows&logoColor=white)

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

im not sure yet

# ai usage
ai was used in the making of this project (claude code)
