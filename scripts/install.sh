#!/usr/bin/env bash
set -euo pipefail

cargo install --path . --locked --force

cargo_bin_dir="${CARGO_HOME:-$HOME/.cargo}/bin"
for shortcut in analyze batch interactive schema health; do
	ln -sf sql-optimizer-cli "${cargo_bin_dir}/${shortcut}"
done

echo "Installed sql-optimizer-cli and shortcut commands to ${cargo_bin_dir}"
