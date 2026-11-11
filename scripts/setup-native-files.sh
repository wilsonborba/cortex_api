#!/bin/sh
# Debian 13 x86_64, matching the MSI runtimes. Writes inside this worktree only.
set -eu
cd "$(dirname "$0")/.."
task_root=$(pwd)
mkdir -p .scratch/native-debs .scratch/native models/pdfium
(
    cd .scratch/native-debs
    apt-get download libtesseract-dev=5.5.0-1+b1 libleptonica-dev=1.84.1-4
)
for package in .scratch/native-debs/*.deb; do
    dpkg-deb -x "$package" .scratch/native
done
native_lib="$task_root/.scratch/native/usr/lib/x86_64-linux-gnu"
ln -sf /usr/lib/x86_64-linux-gnu/libtesseract.so.5 "$native_lib/libtesseract.so"
ln -sf /usr/lib/x86_64-linux-gnu/libleptonica.so.6 "$native_lib/libleptonica.so"
for metadata in "$native_lib"/pkgconfig/*.pc; do
    sed -i "s|^prefix=/usr$|prefix=$task_root/.scratch/native/usr|" "$metadata"
done
# The runtime shared library already links these dependencies.
sed -i 's/^Libs: .*/Libs: -L${libdir} -ltesseract/' "$native_lib/pkgconfig/tesseract.pc"
curl -fL --max-time 120 -o .scratch/pdfium-8125.tgz \
    https://github.com/bblanchon/pdfium-binaries/releases/download/chromium/8125/pdfium-linux-x64.tgz
printf '%s\n' 'e1c3ce6e270cb5e0c20816b379ad9f2c1309972697c505f508de92bf5e7d6659  .scratch/pdfium-8125.tgz' | sha256sum -c -
tar -xzf .scratch/pdfium-8125.tgz -C models/pdfium
