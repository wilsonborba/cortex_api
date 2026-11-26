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
verify_bonsai_assets() {
  local enabled=false
  local enabled_value=${CORTEX_BONSAI_ENABLED:-}
  if [ "${enabled_value,,}" = true ]; then enabled=true; fi
  if [ -f .env ] && python3 -c 'import pathlib,sys; sys.exit(0 if any((p:=line.partition("="))[0].strip()=="CORTEX_BONSAI_ENABLED" and p[2].strip().strip(chr(34)+chr(39)).lower()=="true" for line in pathlib.Path(sys.argv[1]).read_text().splitlines()) else 1)' .env; then
    enabled=true
  fi
  local paths=(
    models/prism/llama-server
    models/bonsai/bonsai25-atomicchat-uncensored-PTQ1_0.gguf
    models/bonsai/bonsai25-official-PTQ1_0-mtp.gguf
  )
  local hashes=(
    89e7f20b23b97bf77b91a0143a168763f7835fd6e0a7a7a23bb70951d991c02c
    f6b5f856c3981ff5ba1db1ce13cdf1ca85f2f701d26ce287e03983b659b681b4
    e0fc5f3d47b9c926d52677d303b19df356a2376614b3e1cb9973865fc04ca431
  )
  for index in "${!paths[@]}"; do
    path=${paths[$index]}
    if [ ! -f "$path" ]; then
      if [ "$enabled" = true ]; then
        printf 'Bonsai is enabled, but required artifact is missing: %s\n' "$path" >&2
        return 1
      fi
      continue
    fi
    actual=$(sha256sum "$path" | cut -d ' ' -f1)
    if [ "$actual" != "${hashes[$index]}" ]; then
      printf 'Bonsai artifact hash mismatch: %s\n' "$path" >&2
      return 1
    fi
    if [ "$index" = 0 ] && [ ! -x "$path" ]; then
      printf 'Prism server is not executable: %s\n' "$path" >&2
      return 1
    fi
  done
}
verify_bonsai_assets
source_hash() {
  git ls-files -z -- Cargo.toml Cargo.lock build.rs src migrations .cargo "scripts/install.sh" |
    LC_ALL=C sort -z | xargs -0 sha256sum | sha256sum | cut -d ' ' -f1
}
if [ "$mode" = --prepare ]; then
  service_user=$(stat -c %U "$root")
  service_group=$(id -gn "$service_user")
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
for unit in "${units[@]}"; do
  grep -Fxq "WorkingDirectory=$root" "var/deploy/$unit.service" || {
    printf 'Prepared units belong to another checkout. Prepare in this checkout.\n' >&2; exit 1;
  }
done
if [ "$mode" = --check ]; then
  printf 'Prepared release verified. No service changed.\n'
  exit 0
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
printf 'Services are active and run the prepared release binaries.\n'
