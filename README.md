# fiberglass

a little trading terminal for polymarket. lives in your terminal. browse
markets, place orders, run a paper account, copy other wallets, poke it from a
script or an ai agent. that's it.

```
   .--------------------------------------------------.
   |  ~ fiberglass ~                                  |
   |                                                  |
   |   [ rust 1.88+ ]   [ macos / linux ]   [ mit ]   |
   |   [ paper trading: yes ]   [ live: careful!! ]   |
   |                                                  |
   |        keyboard-driven. local-first.             |
   '--------------------------------------------------'
```

## heads up

still very much work in progress. do not put in money you can't lose.

live mode signs and sends **real orders with real funds**, and none of it is
battle-tested. start with `--paper`. check every transaction yourself. if you
lose money it's on you.

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

binary is called `fiberglass`.

## the terminal

main way to use it is the tui:

```bash
fiberglass tui            # live mode, needs a wallet
fiberglass tui --paper    # paper mode, $10k fake money, no wallet
```

```
 1 dashboard  2 markets  3 portfolio  4 positions  5 orders ...
 .------------------------------------------------------------.
 |  portfolio   $10,240    cash  $9,120    pnl  +$240         |
 |------------------------------------------------------------|
 |  open positions  3   |  14:02  buy   will btc top 100k  .. |
 |  open orders     2   |  14:01  sell  fed cuts in march  .. |
 '------------------------------------------------------------'
  arrows move . enter open . / search . b buy . s sell . q quit
```

tabs switch with `tab` or `1`-`9`. red frame = live, green = paper. open a
market with `enter`, buy/sell with `b`/`s`, search with `/`. paste a
polymarket.com link into search to jump straight there.

## paper trading

no wallet, no keys, fake balance against live prices.

```bash
fiberglass paper enable
fiberglass paper buy <token> --amount 100
fiberglass paper sell <token> --size 50
fiberglass paper portfolio
fiberglass paper stats
```

market orders walk the real book so you pay real-ish slippage. limit orders
rest until the market crosses them. data sits in
`~/.config/polymarket/paper_account.json`.

## the cli

everything the tui does also works as a plain command. a few:

```bash
fiberglass markets list --limit 5
fiberglass markets search "election"
fiberglass clob book <token>
fiberglass data positions 0xwallet
fiberglass copytrade add 0xwallet --label whale --max-size 50
fiberglass -o json markets list --limit 100   # json for scripts
```

add `-o json` to anything for machine output. run `fiberglass --help` (or
`fiberglass <group> --help`) for the full list — markets, events, clob, ctf,
bridge, wallet, approve, settings, paper, copytrade, and so on.

want a wallet? `fiberglass setup` walks you through it. most read-only
commands don't need one.

## ai agents (mcp)

`fiberglass mcp` is a model context protocol server over stdio. point a client
at it:

```json
{ "mcpServers": { "fiberglass": { "command": "fiberglass", "args": ["mcp"] } } }
```

claude code: `claude mcp add fiberglass -- fiberglass mcp`. paper vs live is
respected, so the same money warning applies.

## config

lives in `~/.config/polymarket/` — wallet, settings, paper account, guards,
copytrade roster. private key is stored in plaintext (0600), so treat the
machine accordingly.

see [changelog.md](CHANGELOG.md) for what changed.

## stars

[![star history](https://api.star-history.com/svg?repos=jesodium/fiberglass&type=Date)](https://star-history.com/#jesodium/fiberglass&Date)

## license

mit
