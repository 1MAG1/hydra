# Update PKGBUILD, generate .SRCINFO via makepkg, and publish to AUR.
#
#   scripts/update-aur.sh [vX.Y.Z]
#
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

VERSION="${1:-}"
if [ -z "$VERSION" ]; then
  VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
fi
VERSION="${VERSION#v}"

echo "Updating AUR packages for version ${VERSION}..."

WORKDIR=$(mktemp -d)
trap 'rm -rf "$WORKDIR"' EXIT

generate_srcinfo() {
  local dir="$1"
  if command -v makepkg >/dev/null 2>&1; then
    (cd "$dir" && makepkg --printsrcinfo > .SRCINFO)
  elif command -v docker >/dev/null 2>&1; then
    docker run --rm -i archlinux:base bash -c "
      cat > /tmp/PKGBUILD
      useradd -m builder
      su builder -c 'cd /tmp && makepkg --printsrcinfo'
    " < "$dir/PKGBUILD" > "$dir/.SRCINFO"
  else
    echo "error: neither makepkg nor docker found to generate .SRCINFO" >&2
    exit 1
  fi
}

update_pkg() {
  local pkg="$1"
  local template_dir="$2"
  local aur_repo="ssh://aur@aur.archlinux.org/${pkg}.git"
  local clone_dir="$WORKDIR/$pkg"

  echo "==> Updating $pkg..."
  git clone "$aur_repo" "$clone_dir"

  # Copy template PKGBUILD
  cp "$template_dir/PKGBUILD" "$clone_dir/PKGBUILD"

  # Update version in PKGBUILD
  perl -pi -e "s/^pkgver=.*/pkgver=${VERSION}/" "$clone_dir/PKGBUILD"
  perl -pi -e "s/^pkgrel=.*/pkgrel=1/" "$clone_dir/PKGBUILD"

  # Generate .SRCINFO with makepkg
  generate_srcinfo "$clone_dir"

  cd "$clone_dir"
  git config user.name "github-actions[bot]"
  git config user.email "github-actions[bot]@users.noreply.github.com"
  git add PKGBUILD .SRCINFO

  if ! git diff --staged --quiet; then
    git commit -m "chore(release): bump to v${VERSION}"
    git push origin master
    echo "==> Pushed update for $pkg"
  else
    echo "==> No changes for $pkg"
  fi
  cd "$REPO_ROOT"
}

update_pkg "hydra-download-manager" "$REPO_ROOT/packaging/aur"
update_pkg "hydra-download-manager-bin" "$REPO_ROOT/packaging/aur-bin"

echo "AUR updates completed."
