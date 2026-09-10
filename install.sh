#!/usr/bin/env bash
# Install everything not-yet-done ships: the binaries *and* the Waybar module.
#
# They belong in one step because the module is a shared object with its own
# copy of the host compiled in, config parser included. Left behind while the
# view files move on, it silently skips every file its older schema cannot
# read and then hides itself, which on the bar is indistinguishable from
# "nothing is running".
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$repo"

waybar_dir="${XDG_CONFIG_HOME:-$HOME/.config}/waybar/cffi"

echo "── binaries ──"
cargo install --path not-yet-done-cli
cargo install --path not-yet-done-tui
# The browser auth plugin, which a user configures by writing its *name* into
# `auth.plugins[].command` — so it has to be on the PATH, not merely built.
# Installed whether or not anybody has configured one: a plugin that is
# missing surfaces as a login failing at the moment somebody needs to log in.
cargo install --path not-yet-done-auth-drunken

echo "── waybar module ──"
cargo build --release -p not-yet-done-waybar

if [ -d "$waybar_dir" ]; then
    install -m 755 target/release/libnyd_waybar.so "$waybar_dir/"
    echo "installed $waybar_dir/libnyd_waybar.so"
    echo "restart the bar to load it (under sway: swaymsg reload)"
else
    echo "no $waybar_dir — skipping the module; create the directory and rerun"
    echo "if your bar is configured for it"
fi
