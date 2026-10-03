#!/usr/bin/env bash
set -euo pipefail
app="$1"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
offset="$($app --appimage-offset)"
test "$offset" -gt 0
head -c "$offset" "$app" > "$tmp/runtime"
unsquashfs -quiet -offset "$offset" -d "$tmp/AppDir" "$app"
hook="$tmp/AppDir/apprun-hooks/linuxdeploy-plugin-gtk.sh"
python3 - "$hook" <<'PY2'
import pathlib, sys
p = pathlib.Path(sys.argv[1])
s = p.read_text()
old = 'export GDK_BACKEND=x11 # Crash with Wayland backend on Wayland - We tested it without it and ended up with this: https://github.com/tauri-apps/tauri/issues/8541'
new = '''if [ -z "${GDK_BACKEND:-}" ]; then
    if [ -n "${WAYLAND_DISPLAY:-}" ]; then
        export GDK_BACKEND=wayland
    else
        export GDK_BACKEND=x11
    fi
fi'''
if old not in s:
    raise SystemExit('expected linuxdeploy GTK hook was not found')
p.write_text(s.replace(old, new))
PY2
for lib in libwayland-client.so.0 libwayland-cursor.so.0 libwayland-egl.so.1 libwayland-server.so.0; do
  find "$tmp/AppDir/usr/lib" -maxdepth 1 -name "$lib" -delete
done
mksquashfs "$tmp/AppDir" "$tmp/squashfs" -quiet -noappend
cat "$tmp/runtime" "$tmp/squashfs" > "$tmp/patched.AppImage"
chmod --reference="$app" "$tmp/patched.AppImage"
mv "$tmp/patched.AppImage" "$app"
