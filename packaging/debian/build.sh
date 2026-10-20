#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
binary=$(realpath "${1:?usage: build.sh BINARY OUTDIR}")
out=$(realpath -m "${2:?usage: build.sh BINARY OUTDIR}")
version=1.0.0
[[ $("$binary" --version) == "arcdeck $version" ]] || { echo 'binary version mismatch' >&2; exit 1; }
arch=$(dpkg --print-architecture)
[[ $arch == amd64 || $arch == arm64 ]] || { echo "unsupported architecture: $arch" >&2; exit 1; }
mkdir -p "$out"
stage=$(mktemp -d)
trap 'rm -rf -- "$stage"' EXIT
install -Dm755 "$binary" "$stage/usr/bin/arcdeck"
install -Dm644 LICENSE "$stage/usr/share/doc/arc-deck/LICENSE"
install -Dm644 NOTICE "$stage/usr/share/doc/arc-deck/NOTICE"
install -Dm644 crates/arc-deck-tui/LICENSE-OpenCode "$stage/usr/share/doc/arc-deck/LICENSE-OpenCode"
install -Dm644 docs/INSTALL.md "$stage/usr/share/doc/arc-deck/INSTALL.md"
cat NOTICE LICENSE crates/arc-deck-tui/LICENSE-OpenCode > "$stage/usr/share/doc/arc-deck/copyright"
mkdir -p "$stage/DEBIAN" "$stage/debian"
# dpkg-shlibdeps reads the ELF's actual imports, avoiding guessed libc dependencies.
printf 'Source: arc-deck\n\nPackage: arc-deck\nArchitecture: any\nDescription: Arc Deck\n' > "$stage/debian/control"
deps=$(cd "$stage" && dpkg-shlibdeps -O -eusr/bin/arcdeck | sed -n 's/^shlibs:Depends=//p')
[[ -n $deps ]] || { echo 'could not determine shared library dependencies' >&2; exit 1; }
rm -r -- "$stage/debian"
cat > "$stage/DEBIAN/control" <<EOF
Package: arc-deck
Version: $version-1
Section: devel
Priority: optional
Architecture: $arch
Maintainer: Takahashi Rinta <mc1586182829@outlook.com>
Depends: $deps, nodejs (>= 24), npm, git
Installed-Size: $(du -sk "$stage/usr" | cut -f1)
Homepage: https://github.com/ProjectArcturius/arc-deck
Description: DSH-powered coding agent with TUI and headless workers
 Arc Deck provides interactive coding conversations and supervised workers.
 Install the pinned DSH runtime separately using arcdeck runtime install.
EOF
dpkg-deb --root-owner-group --build "$stage" "$out/arc-deck_${version}-1_${arch}.deb"
