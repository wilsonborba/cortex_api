#!/usr/bin/env bash
# Prepare and install verified release artifacts. Never migrate a database.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
root=$(pwd)
mode=${1:---prepare}
case "$mode" in --prepare|--check|--activate) ;; *) printf 'Use install.sh [--prepare|--check|--activate]\n' >&2; exit 1;; esac
if [ "$mode" = --activate ] && [ "$(id -u)" != 0 ]; then
  printf 'Administrator rights are required to activate the system services.\n' >&2
  exit 1
fi
mkdir -p var
exec 9>var/deploy.lock
flock -n 9 || { printf 'Another deployment is in progress.\n' >&2; exit 1; }
binaries=(cortex-api)
units=(cortex-api)
prism_enabled=false
if [ "${CORTEX_BONSAI_ENABLED:-}" ] && [ "${CORTEX_BONSAI_ENABLED,,}" = true ]; then
  prism_enabled=true
elif [ -f .env ] && python3 -c 'import pathlib,sys; sys.exit(0 if any((p:=line.partition("="))[0].strip()=="CORTEX_BONSAI_ENABLED" and p[2].strip().strip(chr(34)+chr(39)).lower()=="true" for line in pathlib.Path(sys.argv[1]).read_text().splitlines()) else 1)' .env; then
  prism_enabled=true
fi
source_hash() {
  git ls-files -z -- Cargo.toml Cargo.lock build.rs src migrations .cargo scripts/install.sh scripts/stage_prism.py scripts/activate_prism.py deploy/prism |
    LC_ALL=C sort -z | xargs -0 sha256sum | sha256sum | cut -d ' ' -f1
}
if [ "$mode" = --prepare ]; then
  service_user=$(stat -c %U "$root")
  service_group=$(id -gn "$service_user")
  if [ "$prism_enabled" = true ]; then
    : "${CORTEX_PRISM_RUNTIME_SOURCE:?Set CORTEX_PRISM_RUNTIME_SOURCE to the verified Prism runtime directory}"
    : "${CORTEX_BONSAI_MODEL_SOURCE:?Set CORTEX_BONSAI_MODEL_SOURCE to the directory containing the two pinned Bonsai models}"
    [ -d "$CORTEX_PRISM_RUNTIME_SOURCE" ] || { printf 'Prism runtime source directory is missing.\n' >&2; exit 1; }
    [ -d "$CORTEX_BONSAI_MODEL_SOURCE" ] || { printf 'Bonsai model source directory is missing.\n' >&2; exit 1; }
  fi
  before=$(source_hash)
  cargo build --release --locked --bins -j2
  [ "$before" = "$(source_hash)" ] || {
    printf 'Source changed during the build. Prepare again.\n' >&2; exit 1;
  }
  target=$(cargo metadata --no-deps --locked --format-version 1 |
    python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
  stage=$(mktemp -d "$root/var/deploy.XXXXXX")
  trap 'rm -rf "$stage"' EXIT
  for binary in "${binaries[@]}"; do
    install -m 755 "$target/release/$binary" "$stage/$binary"
  done
  cat > "$stage/cortex-api.service" <<UNIT
[Unit]
Description=Cortex Rust API
After=network-online.target hippocampus-api.service
Wants=network-online.target

[Service]
Type=simple
User=$service_user
Group=$service_group
WorkingDirectory=$root
EnvironmentFile=$root/.env
ExecStart=$root/target/release/cortex-api --production
Restart=on-failure
RestartSec=3
NoNewPrivileges=yes
PrivateTmp=yes
UMask=027

[Install]
WantedBy=multi-user.target
UNIT
  if [ "$prism_enabled" = true ]; then
    python3 scripts/stage_prism.py stage "$stage/prism" \
      --runtime "$CORTEX_PRISM_RUNTIME_SOURCE" \
      --models "$CORTEX_BONSAI_MODEL_SOURCE" \
      --cortex-user "$service_user"
  fi
  source_hash > "$stage/source.sha256"
  (cd "$stage"; sha256sum "${binaries[@]}" *.service > artifacts.sha256)
  rm -rf var/deploy
  mv "$stage" var/deploy
  trap - EXIT
  printf 'Release binaries and service files prepared in var/deploy.\n'
  exit 0
fi
if [ ! -f var/deploy/artifacts.sha256 ] || [ ! -f var/deploy/source.sha256 ]; then
  printf 'No prepared release. Run install.sh --prepare first.\n' >&2
  exit 1
fi
[ "$(source_hash)" = "$(cat var/deploy/source.sha256)" ] || {
  printf 'Source changed. Run install.sh --prepare again.\n' >&2; exit 1;
}
(cd var/deploy; sha256sum --check --status artifacts.sha256) || {
  printf 'Prepared artifacts changed. Run install.sh --prepare again.\n' >&2; exit 1;
}
if [ -d var/deploy/prism ]; then
  python3 scripts/stage_prism.py check var/deploy/prism || {
    printf 'Prepared Prism assets changed or failed verification. Run install.sh --prepare again.\n' >&2; exit 1;
  }
elif [ "$prism_enabled" = true ]; then
  printf 'Prism is enabled, but the prepared release has no Prism artifacts. Run install.sh --prepare.\n' >&2
  exit 1
fi
for unit in "${units[@]}"; do
  grep -Fxq "WorkingDirectory=$root" "var/deploy/$unit.service" || {
    printf 'Prepared units belong to another checkout. Prepare in this checkout.\n' >&2; exit 1;
  }
done
if [ "$mode" = --check ]; then
  printf 'Prepared release verified. No service changed.\n'
  exit 0
fi
if [ -d var/deploy/prism ] && systemctl is-active --quiet prism.service; then
  printf 'Prism is active. Wait for its shared GPU permit to be released before Cortex service activation.\n' >&2
  exit 1
fi
mkdir -p target/release
for binary in "${binaries[@]}"; do
  install -m 755 "var/deploy/$binary" "target/release/$binary.new"
  mv -f "target/release/$binary.new" "target/release/$binary"
done
for unit in "${units[@]}"; do
  install -m 644 "var/deploy/$unit.service" "/etc/systemd/system/$unit.service"
done
systemctl daemon-reload
for unit in "${units[@]}"; do systemctl enable "$unit.service"; done
systemctl restart "${units[@]/%/.service}"
for index in "${!units[@]}"; do
  unit=${units[$index]}
  binary=${binaries[$index]}
  expected=$(sha256sum "var/deploy/$binary" | cut -d ' ' -f1)
  matched=false
  for attempt in {1..30}; do
    pid=$(systemctl show "$unit.service" --property=MainPID --value)
    if systemctl is-active --quiet "$unit.service" && [ "$pid" != 0 ] &&
       [ "$(sha256sum "/proc/$pid/exe" 2>/dev/null | cut -d ' ' -f1)" = "$expected" ]; then
      matched=true; break
    fi
    sleep 0.2
  done
  if [ "$matched" != true ]; then
    printf 'Running binary verification failed for %s.\n' "$unit.service" >&2; exit 1
  fi
done
if [ -d var/deploy/prism ]; then
  python3 scripts/activate_prism.py var/deploy/prism
fi
printf 'Services are active and run the prepared release binaries.\n'
