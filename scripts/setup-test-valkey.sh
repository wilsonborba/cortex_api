#!/bin/sh
# Test binary only. No package install and no service action.
set -eu
cd "$(dirname "$0")/.."
mkdir -p .scratch/valkey-debs .scratch/valkey
(
    cd .scratch/valkey-debs
    apt-get download valkey-server=8.1.1+dfsg1-3+deb13u2 valkey-tools=8.1.1+dfsg1-3+deb13u2
)
for package in .scratch/valkey-debs/*.deb; do
    dpkg-deb -x "$package" .scratch/valkey
done
