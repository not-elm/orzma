#!/bin/sh
# Installs this orzma release for the current user; no root access is needed.
set -eu

src_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)
data_home=${XDG_DATA_HOME:-$HOME/.local/share}
app_dir=$data_home/orzma
bin_dir=$HOME/.local/bin
link=$bin_dir/orzma
sizes="48 128 256 512"
newline='
'
carriage_return=$(printf '\r')

fail() {
    echo "install.sh: $*" >&2
    exit 1
}

required="orzma libcef.so share/applications/orzma.desktop"
for size in $sizes; do
    required="$required share/icons/hicolor/${size}x${size}/apps/orzma.png"
done
for entry in $required; do
    [ -e "$src_dir/$entry" ] || fail "$src_dir/$entry not found; run this script from an extracted orzma release"
done

# NOTE: the path is written into the desktop entry's quoted Exec value, where
# these characters would need escaping that sed cannot express safely. Every
# check runs before anything is copied, so a rejected path changes nothing.
case $app_dir in
    *'"'* | *'`'* | *'$'* | *'\'* | *'%'* | *'|'* | *'&'* | *"$newline"* | *"$carriage_return"*)
        fail "unsupported character in install path $app_dir; set XDG_DATA_HOME to a plain path" ;;
esac

if [ -e "$link" ] && [ ! -L "$link" ]; then
    fail "$link exists and is not a symlink; move it away and rerun"
fi

# NOTE: compare by inode with -ef, never by string: a trailing slash in
# XDG_DATA_HOME or a symlinked HOME spells the same directory differently, and
# replacing the tree the installed copy runs from would delete it.
if ! { [ -d "$app_dir" ] && [ "$src_dir" -ef "$app_dir" ]; }; then
    staging=$app_dir.tmp.$$
    rm -rf "$staging"
    mkdir -p "$staging"
    cp -R "$src_dir/." "$staging/" || { rm -rf "$staging"; fail "copying the release into $staging failed"; }
    rm -rf "$app_dir"
    mv "$staging" "$app_dir"
fi

mkdir -p "$bin_dir"
ln -sfn "$app_dir/orzma" "$link"

mkdir -p "$data_home/applications"
sed "s|@ORZMA_EXEC@|\"$app_dir/orzma\"|" "$app_dir/share/applications/orzma.desktop" \
    > "$data_home/applications/orzma.desktop"

for size in $sizes; do
    icon_dir=$data_home/icons/hicolor/${size}x${size}/apps
    mkdir -p "$icon_dir"
    cp "$app_dir/share/icons/hicolor/${size}x${size}/apps/orzma.png" "$icon_dir/orzma.png"
done

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$data_home/applications" || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t "$data_home/icons/hicolor" || true
fi

echo "orzma installed to $app_dir"
case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *) echo "note: $bin_dir is not on PATH; add it to run orzma from a shell" ;;
esac
