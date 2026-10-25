#!/bin/sh
# Make the public copy of kimo: the current commit as a single commit, without private files.
#   scripts/export-public.sh <empty folder>
set -eu
dest="${1:?usage: scripts/export-public.sh <empty folder>}"
if [ -e "$dest" ] && [ -n "$(ls -A "$dest")" ]; then
  echo "$dest is not empty" >&2
  exit 1
fi
root="$(git rev-parse --show-toplevel)"
mkdir -p "$dest"
git -C "$root" archive --format=tar HEAD | tar -x -C "$dest"
# Private: the author's project-manager metadata.
rm -rf "$dest/.pm"
# The commercial web theme used early in the private history must not be in the public copy.
if grep -rIil -e "timber" -e "entypo" "$dest" | grep -v "/scripts/export-public.sh$"; then
  echo "the files above still mention the old web theme" >&2
  exit 1
fi
cd "$dest"
git init -q -b main
git add -A
git -c user.name="$(git -C "$root" config user.name)" -c user.email="$(git -C "$root" config user.email)" \
  commit -q -m "kimo 0.1.0"
echo "public copy ready in $dest ($(git ls-files | wc -l) files, one commit)"
