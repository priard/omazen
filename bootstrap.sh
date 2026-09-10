#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-only
# See NOTICE for the required Omazen project attribution terms.
#
# One-step install on Omarchy: downloads the latest Omazen release, checks it
# against its SHA-256 sidecar, installs missing packages and runs install.sh,
# which also sets up Zen web apps.
#
#   curl -fsSL https://raw.githubusercontent.com/priard/omazen/main/bootstrap.sh | bash
#
# OMAZEN_REPO=owner/name and OMAZEN_RELEASE=vX.Y.Z pick another fork or release.

set -euo pipefail

REPO=${OMAZEN_REPO:-priard/omazen}
RELEASE=${OMAZEN_RELEASE:-latest}

say() { printf '\033[32m==>\033[0m %s\n' "$*"; }
die() {
  printf 'ERROR: %s\n' "$*" >&2
  exit 1
}

# Piped into bash, stdin is this script; prompts (sudo, pacman, setup) read
# the terminal instead.
tty=/dev/stdin
if [[ ! -t 0 ]] && { : </dev/tty; } 2>/dev/null; then
  tty=/dev/tty
fi

[[ $(uname -m) == x86_64 ]] || die "Omazen releases are built for x86-64 only"
command -v omarchy >/dev/null 2>&1 || die "Omazen needs Omarchy 4 (Quattro)"
for tool in curl tar sha256sum; do
  command -v "$tool" >/dev/null 2>&1 || die "$tool is required"
done

# inotify-tools drives the event-driven theme watcher and gum asks for web app
# details; Zen itself comes from the AUR.
missing=()
for package in inotify-tools gum; do
  pacman -Q "$package" >/dev/null 2>&1 || missing+=("$package")
done
if ((${#missing[@]})); then
  say "Installing ${missing[*]}"
  omarchy pkg add "${missing[@]}" <"$tty"
fi
if ! pacman -Q zen-browser-bin >/dev/null 2>&1; then
  say "Installing zen-browser-bin"
  omarchy pkg aur add zen-browser-bin <"$tty" || die "could not install zen-browser-bin"
fi

if [[ $RELEASE == latest ]]; then
  api="https://api.github.com/repos/$REPO/releases/latest"
else
  api="https://api.github.com/repos/$REPO/releases/tags/$RELEASE"
fi
tag=$(curl -fsSL "$api" 2>/dev/null | sed -nE 's/^ *"tag_name": *"([^"]+)".*/\1/p' | head -n 1) || true
[[ $tag =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "no Omazen release found at $api"
version=${tag#v}
package="omazen-$version-linux-x86_64"
base="https://github.com/$REPO/releases/download/$tag"

work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
say "Downloading Omazen $version"
for file in "$package.tar.gz" "$package.tar.gz.sha256"; do
  curl -fsSL -o "$work/$file" "$base/$file" || die "could not download $base/$file"
done
(cd "$work" && sha256sum --check --quiet "$package.tar.gz.sha256") ||
  die "checksum mismatch for $package.tar.gz"
tar -xzf "$work/$package.tar.gz" -C "$work"

say "Installing Omazen $version"
(cd "$work/$package" && ./install.sh) <"$tty"
say "Done. Close Zen normally and open it again once; theme changes are live after that."
