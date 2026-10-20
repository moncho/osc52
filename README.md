# osc52

Copy text to the system clipboard through the terminal with the OSC 52
escape sequence. Because the terminal does the copying, it works over SSH
and inside tmux or screen. It is built without the Rust standard library and
links only libc, so the binary is about 50 KB.

```sh
osc52 some text          # copy the arguments, joined by spaces
git rev-parse HEAD | osc52   # copy standard input
```

The sequence is written to `/dev/tty`, so it reaches the terminal even when
stdout is redirected. Inside screen it is split into pieces that fit screen's
string limit. Over 1 MB, `osc52` still sends it but warns: tmux drops and
iTerm2 truncates sequences that long.

## Install

On Linux or macOS, without root, into `~/.local/bin`:

```sh
curl -fsSL https://raw.githubusercontent.com/moncho/osc52/main/install.sh | sh
```

The script downloads the static binary for your architecture from the latest
release and checks it against its SHA-256 before installing. Set
`OSC52_INSTALL_DIR` to install elsewhere, or `OSC52_VERSION` (such as
`v0.3.0`) to install a specific release.

With Homebrew, on macOS or Linux:

```sh
brew install moncho/tap/osc52
```

On Debian, Ubuntu and other `.deb` distributions (use `arm64` for ARM):

```sh
curl -fLO https://github.com/moncho/osc52/releases/latest/download/osc52_amd64.deb
sudo apt install ./osc52_amd64.deb
```

On Fedora, RHEL and other `dnf`-based distributions (use `aarch64` for ARM):

```sh
sudo dnf install https://github.com/moncho/osc52/releases/latest/download/osc52.x86_64.rpm
```

Or download a binary for your platform from the
[latest release](https://github.com/moncho/osc52/releases/latest):
`osc52-x86_64-unknown-linux-musl`, `osc52-aarch64-unknown-linux-musl`,
`osc52-x86_64-apple-darwin` or `osc52-aarch64-apple-darwin`. Each has a
`.sha256` file next to it. The Linux binaries are statically linked.

```sh
target=aarch64-apple-darwin
curl -fLO "https://github.com/moncho/osc52/releases/latest/download/osc52-$target"
curl -fLO "https://github.com/moncho/osc52/releases/latest/download/osc52-$target.sha256"
shasum -a 256 -c "osc52-$target.sha256"
install -m 755 "osc52-$target" ~/.local/bin/osc52
```

Or build from source:

```sh
cargo install --git https://github.com/moncho/osc52
```

## Terminal setup

Your terminal must allow OSC 52 clipboard writes. Ghostty, kitty, WezTerm,
iTerm2 and Alacritty support it; some need it enabled in their settings.
GNOME Terminal and other VTE-based terminals do not.

To ask the terminal, run this outside tmux and screen:

```sh
osc52 --check
```

It exits 0 when the terminal says it supports OSC 52, 1 when it does not, and
2 when it cannot tell. Some terminals, such as Alacritty and Konsole, support
OSC 52 without saying so; copy something and paste it to find out.

In tmux, enable passthrough:

```
set -g allow-passthrough on
```
