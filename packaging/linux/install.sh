#!/bin/sh
# Installs Petramond for the current user: copies this folder to
# ~/.local/share/petramond-game and adds the game to the applications menu.
#
#   ./install.sh              install, or update an earlier install
#   ./install.sh --uninstall  remove the game (worlds and settings are kept)
#
# Nothing here needs root, and nothing is distro-specific: the menu entry and
# icon follow the freedesktop.org rules every mainstream desktop reads.
set -eu

DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
DEST="$DATA/petramond-game"
DESKTOP="$DATA/applications/petramond.desktop"
ICONS="$DATA/icons/hicolor"
LINK="$HOME/.local/bin/petramond"
ICON_SIZES="16 32 48 64 128 256 512"

SRC="$(cd "$(dirname "$0")" && pwd -P)"

refresh_menus() {
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database -q "$DATA/applications" 2>/dev/null || true
    fi
    if command -v gtk-update-icon-cache >/dev/null 2>&1; then
        gtk-update-icon-cache -q -t "$ICONS" 2>/dev/null || true
    fi
}

uninstall() {
    rm -rf "$DEST"
    rm -f "$DESKTOP"
    for size in $ICON_SIZES; do
        rm -f "$ICONS/${size}x${size}/apps/petramond.png"
    done
    if [ -L "$LINK" ]; then
        rm -f "$LINK"
    fi
    refresh_menus
    echo "Petramond has been removed. Your worlds and settings were left in place."
}

# Quotes a path for a .desktop Exec line.
desktop_quote() {
    printf '"%s"' "$(printf '%s' "$1" | sed -e 's/[\\"`$]/\\&/g' -e 's/%/%%/g')"
}

case "${1:-}" in
    "") ;;
    --uninstall)
        uninstall
        exit 0
        ;;
    *)
        echo "usage: $0 [--uninstall]" >&2
        exit 2
        ;;
esac

if [ ! -x "$SRC/petramond" ]; then
    echo "Run this script from the extracted Petramond folder." >&2
    exit 1
fi
if [ "$SRC" = "$(cd "$DEST" 2>/dev/null && pwd -P)" ]; then
    echo "This copy is already the installed one. To update, run install.sh from a newly extracted download." >&2
    exit 1
fi

# Copy to a side folder first, so a failed copy never leaves a half-updated install.
rm -rf "$DEST.new"
mkdir -p "$DEST.new"
cp -R "$SRC/." "$DEST.new/"
rm -rf "$DEST"
mv "$DEST.new" "$DEST"

for size in $ICON_SIZES; do
    mkdir -p "$ICONS/${size}x${size}/apps"
    cp "$DEST/icons/petramond-$size.png" "$ICONS/${size}x${size}/apps/petramond.png"
done

mkdir -p "$(dirname "$DESKTOP")"
cat > "$DESKTOP" <<EOF
[Desktop Entry]
Type=Application
Name=Petramond
GenericName=Voxel Sandbox
Comment=Explore wild landscapes and build a home
Exec=$(desktop_quote "$DEST/petramond")
Icon=petramond
Terminal=false
Categories=Game;Simulation;
Keywords=voxel;sandbox;blocks;building;
StartupWMClass=petramond
StartupNotify=true
EOF

mkdir -p "$(dirname "$LINK")"
if [ -e "$LINK" ] && [ ! -L "$LINK" ]; then
    echo "Skipped $LINK: a different file is already there."
else
    ln -sf "$DEST/petramond" "$LINK"
fi

refresh_menus

echo "Petramond is installed. Open it from your applications menu, or run: petramond"
echo "To remove it later, run: $DEST/install.sh --uninstall"
