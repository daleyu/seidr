# Seidr: use this Herdr fork

Seidr still builds an executable named `herdr`; these are the commands used for
our Mac switch, with shell equivalents for the backup/install script.
Requires Rust 1.96.1 and Zig 0.16.0.

## 1. Build

Compile this checkout into an optimized executable.

```sh
cd /Users/daleyu/stripe/seidr
cargo build --release --locked
./target/release/herdr --version
```

## 2. Back up (first switch)

Save the current binary, shell configuration, and Herdr settings before replacing them.

```sh
seidr_backup="$HOME/.local/share/seidr/backups/$(date +%Y%m%d-%H%M%S)"
mkdir -p "$seidr_backup"
chmod 700 "$seidr_backup"
cp "$(command -v herdr)" "$seidr_backup/herdr-before-seidr"
cp "$HOME/.zshrc" "$seidr_backup/zshrc"
cp "$HOME/.config/herdr/config.toml" "$seidr_backup/config.toml"
tar --exclude='*.sock' -czf "$seidr_backup/config.tar.gz" -C "$HOME/.config" herdr
if [ -d "$HOME/.local/state/herdr" ]; then
  tar --exclude='*.sock' -czf "$seidr_backup/state.tar.gz" -C "$HOME/.local/state" herdr
fi
```

## 3. Install

Replace the local executable through a temporary file, leaving the mise installation available for rollback.

```sh
mkdir -p "$HOME/.local/bin"
install -m 755 target/release/herdr "$HOME/.local/bin/herdr.seidr-next" &&
  mv -f "$HOME/.local/bin/herdr.seidr-next" "$HOME/.local/bin/herdr"
cmp target/release/herdr "$HOME/.local/bin/herdr"
```

## 4. Configure (already done on this Mac)

Put the fork first on PATH and disable upstream version notifications.

```sh
# Add once, at the end of ~/.zshrc:
export PATH="$HOME/.local/bin:$PATH"
```

```toml
# Merge into ~/.config/herdr/config.toml:
[update]
version_check = false
```

## 5. Switch the running server

Use live handoff to move existing panes to the new server without stopping their processes.

```sh
"$HOME/.local/bin/herdr" status --json
"$HOME/.local/bin/herdr" server live-handoff \
  --import-exe "$HOME/.local/bin/herdr" \
  --expected-protocol 22 --expected-version 0.9.5
```

This is the experimental handoff that succeeded for our 0.9.3 → 0.9.5 switch;
confirm `live_handoff: true` before using it, and update the expected protocol/version
when they change.

## 6. Verify and reconnect

Confirm a fresh shell selects the fork and the server reports the new version, then detach with **Ctrl+B, Q** and run `herdr` to reopen the client.

```sh
/bin/zsh -lic 'command -v herdr; herdr --version'
"$HOME/.local/bin/herdr" status --json
```

After future edits, repeat **1, 3, 5, and 6**; installing a binary alone does not
update an already-running server. Avoid `herdr update`, which still uses upstream releases.

Our original backup is `~/.local/share/seidr/backups/20261101-174258`.
Devboxes need a separate Linux build and server switch; updating this Mac does not update them.

[Development instructions](AGENTS.md) · [Upstream docs](https://herdr.dev/docs/) · [License](LICENSE)
