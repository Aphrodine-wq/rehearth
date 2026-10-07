#!/bin/sh
# Pushes rehearth and rehearth-bin to the AUR for the version in launcher/Cargo.toml.
# Run it after that version's GitHub release has finished building.
# Needs an AUR account with this machine's SSH key (~/.ssh/aur) added to it.
set -eu
cd "$(dirname "$0")"
version=$(grep -m1 '^version' ../../launcher/Cargo.toml | cut -d'"' -f2)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

for pkg in rehearth rehearth-bin; do
    if ! grep -q "^pkgver=$version$" "$pkg/PKGBUILD"; then
        sed -i "s/^pkgver=.*/pkgver=$version/; s/^pkgrel=.*/pkgrel=1/" "$pkg/PKGBUILD"
    fi
    (cd "$pkg" && updpkgsums && makepkg --printsrcinfo > .SRCINFO && rm -f ./*.tar.gz)
    git clone -q "ssh://aur@aur.archlinux.org/$pkg.git" "$work/$pkg"
    cp "$pkg/PKGBUILD" "$pkg/.SRCINFO" "$work/$pkg/"
    (
        cd "$work/$pkg"
        git add PKGBUILD .SRCINFO
        if git diff --cached --quiet; then
            echo "$pkg: already up to date"
        else
            git commit -q -m "Update to $version" ${AUR_TRAILER:+-m "$AUR_TRAILER"}
            git push -q origin HEAD:master
            echo "$pkg: pushed $version"
        fi
    )
done
