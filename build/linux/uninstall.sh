#!/bin/sh
# Removes a per-user orzma installation made by install.sh; ~/.config/orzma is kept.
set -eu

data_home=${XDG_DATA_HOME:-$HOME/.local/share}
app_dir=$data_home/orzma
link=$HOME/.local/bin/orzma
sizes="48 128 256 512"

if [ -L "$link" ] && [ "$(readlink "$link")" = "$app_dir/orzma" ]; then
    rm -f "$link"
fi
rm -f "$data_home/applications/orzma.desktop"
for size in $sizes; do
    rm -f "$data_home/icons/hicolor/${size}x${size}/apps/orzma.png"
done
rm -rf "$app_dir"

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$data_home/applications" || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t "$data_home/icons/hicolor" || true
fi

echo "orzma uninstalled"
