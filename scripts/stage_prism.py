#!/usr/bin/env python3
"""Stage and verify the pinned Prism runtime, model assets, and host templates."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import sys

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "deploy/prism/runtime-manifest.json"
DEPLOY = ROOT / "deploy/prism"
RUNTIME_RELEASE = "prism-b12178-03ec600"
RUNTIME_UPSTREAM_COMMIT_PREFIX = "03ec60072"
RUNTIME_ARCHIVE_SHA256 = "626dc154204531df146a428d455b53c64534b8f022e98cb0760881b48a35557e"
RUNTIME_SERVER_SHA256 = "89e7f20b23b97bf77b91a0143a168763f7835fd6e0a7a7a23bb70951d991c02c"
RUNTIME_INVENTORY_SHA256 = "d045242cb4072969d3270fa5b885bc6ad333ee28e62875bf07e7cf678b3f684d"
MODELS = {
    "bonsai25-atomicchat-uncensored-PTQ1_0.gguf": (5993834848, "f6b5f856c3981ff5ba1db1ce13cdf1ca85f2f701d26ce287e03983b659b681b4"),
    "bonsai25-official-PTQ1_0-mtp.gguf": (6232818944, "e0fc5f3d47b9c926d52677d303b19df356a2376614b3e1cb9973865fc04ca431"),
}
DEPLOYMENTS = {
    "prism.service": ("/etc/systemd/system/prism.service", 0o644),
    "prism-launch": ("/usr/local/bin/prism-launch", 0o755),
    "prism-tmpfiles.conf": ("/etc/tmpfiles.d/prism.conf", 0o644),
    "49-prism-systemd.rules": ("/etc/polkit-1/rules.d/49-prism-systemd.rules", 0o644),
}
PROFILE_BINDINGS = {
    "bonsai25-uncensored": ("/var/lib/prism/models/bonsai/bonsai25-atomicchat-uncensored-PTQ1_0.gguf", "f6b5f856c3981ff5ba1db1ce13cdf1ca85f2f701d26ce287e03983b659b681b4", "bonsai25-uncensored"),
    "bonsai25-official": ("/var/lib/prism/models/bonsai/bonsai25-official-PTQ1_0-mtp.gguf", "e0fc5f3d47b9c926d52677d303b19df356a2376614b3e1cb9973865fc04ca431", "bonsai25-official"),
}


def digest(path):
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def load_manifest():
    if hashlib.sha256(MANIFEST.read_bytes()).hexdigest() != RUNTIME_INVENTORY_SHA256:
        raise ValueError("Prism runtime inventory does not match the independently verified pin")
    data = json.loads(MANIFEST.read_text())
    if (data.get("schema_version") != "1.0.0" or data.get("release") != RUNTIME_RELEASE
            or data.get("upstream_commit_prefix") != RUNTIME_UPSTREAM_COMMIT_PREFIX
            or data.get("archive_sha256") != RUNTIME_ARCHIVE_SHA256):
        raise ValueError("unsupported pinned Prism runtime manifest")
    names = [e.get("path") for e in data.get("entries", [])]
    if not names or len(names) != len(set(names)) or any(not n or Path(n).name != n for n in names):
        raise ValueError("invalid runtime inventory paths")
    validate_contract(data)
    return data


def validate_contract(runtime_manifest):
    contract = json.loads((DEPLOY / "manifest.json").read_text())
    service = contract.get("service", {})
    if service != {"unit_name": "prism.service", "loopback_address": "127.0.0.1", "fixed_port": 49100, "cortex_binding": "cortex-api.service"}:
        raise ValueError("Prism service binding differs from the approved fixed loopback contract")
    profiles = contract.get("profiles", {})
    if set(profiles) != set(PROFILE_BINDINGS):
        raise ValueError("Prism profile allowlist differs from the two approved profiles")
    for name, binding in PROFILE_BINDINGS.items():
        profile = profiles[name]
        if (profile.get("model_path"), profile.get("model_sha256"), profile.get("runtime_alias")) != binding:
            raise ValueError(f"Prism profile binding mismatch: {name}")
    server = contract.get("runtime_server", {})
    entry = next((item for item in runtime_manifest["entries"] if item["path"] == "llama-server"), None)
    if (not entry or entry["sha256"] != RUNTIME_SERVER_SHA256
            or server != {"path": "/opt/prism/current/llama-server", "sha256": RUNTIME_SERVER_SHA256}):
        raise ValueError("Prism server path or pinned hash differs from the runtime inventory")
    staged = contract.get("staged_files", [])
    if len(staged) != len(DEPLOYMENTS):
        raise ValueError("Prism deployment manifest is missing fixed system files")
    for item in staged:
        name = Path(item.get("path", "")).name
        if item.get("path") != f"deploy/prism/{name}" or name not in DEPLOYMENTS:
            raise ValueError("Prism deployment manifest contains an unexpected source path")
        target, mode = DEPLOYMENTS[name]
        source = DEPLOY / name
        if item.get("target_path") != target or item.get("mode") != f"{mode:04o}" or item.get("sha256") != digest(source):
            raise ValueError(f"Prism deployment source pin mismatch: {name}")
    launcher = (DEPLOY / "prism-launch").read_text()
    unit = (DEPLOY / "prism.service").read_text()
    if "unset LLAMA_API_KEY" not in launcher or "unset LLAMA_ARG_API_KEY_FILE" not in launcher or "PRISM_API_KEY" in launcher:
        raise ValueError("Prism launcher must not configure internal API-key authentication")
    required_unit = ("User=prism", "Group=prism", "BindsTo=cortex-api.service", "After=cortex-api.service", "Restart=no", "KillMode=control-group", "TimeoutStopSec=30", "NoNewPrivileges=yes")
    policy = (DEPLOY / "49-prism-systemd.rules").read_text()
    if ("--host 127.0.0.1" not in launcher or any(value not in unit for value in required_unit)
            or "[Install]" in unit or "unit === \"prism.service\"" not in policy
            or "verb === \"start\"" not in policy or "verb === \"stop\"" not in policy):
        raise ValueError("Prism must remain loopback-only, on-demand, and not enabled at boot")


def check_runtime(source, entries):
    source = source.resolve(strict=True)
    expected = {e["path"] for e in entries}
    actual = {p.name for p in source.iterdir()}
    if actual != expected:
        raise ValueError("runtime inventory does not match the pinned file list")
    for entry in entries:
        path = source / entry["path"]
        if path.is_symlink():
            target = os.readlink(path)
            if target != entry.get("target") or Path(target).name != target:
                raise ValueError(f"runtime symlink target mismatch: {path.name}")
            resolved = path.resolve(strict=True)
            if resolved.parent != source:
                raise ValueError(f"runtime symlink escapes bundle: {path.name}")
            if resolved.stat().st_size != entry["bytes"] or digest(resolved) != entry["sha256"]:
                raise ValueError(f"runtime symlink target hash mismatch: {path.name}")
        else:
            mode = path.lstat().st_mode
            if not stat.S_ISREG(mode) or path.stat().st_size != entry["bytes"] or digest(path) != entry["sha256"]:
                raise ValueError(f"runtime file hash or type mismatch: {path.name}")
            if path.name == "llama-server" and not os.access(path, os.X_OK):
                raise ValueError("pinned Prism server is not executable")


def check_models(source, exact=False):
    source = source.resolve(strict=True)
    actual = {p.name for p in source.iterdir()}
    if not set(MODELS).issubset(actual) or (exact and actual != set(MODELS)):
        raise ValueError("model inventory does not match the two approved profiles")
    for name, (size, sha) in MODELS.items():
        path = source / name
        if path.is_symlink() or not path.is_file() or path.stat().st_size != size or digest(path) != sha:
            raise ValueError(f"model hash or type mismatch: {name}")


def stage(runtime_source, model_source, destination, cortex_user):
    manifest = load_manifest()
    entries = manifest["entries"]
    check_runtime(runtime_source, entries)
    check_models(model_source)
    destination.mkdir(parents=True, exist_ok=False)
    release = destination / "releases" / manifest["release"]
    release.mkdir(parents=True)
    for entry in entries:
        src, dst = Path(runtime_source) / entry["path"], release / entry["path"]
        if entry["symlink"]:
            dst.symlink_to(entry["target"])
        else:
            shutil.copy2(src, dst)
            dst.chmod(0o755 if os.access(src, os.X_OK) else 0o644)
    (destination / "current").symlink_to(f"releases/{manifest['release']}")
    models = destination / "models" / "bonsai"
    models.mkdir(parents=True)
    for name in MODELS:
        shutil.copy2(Path(model_source) / name, models / name)
        (models / name).chmod(0o644)
    system = destination / "system"
    system.mkdir()
    (destination / "cortex-user.txt").write_text(cortex_user + "\n")
    for name, (_, mode) in DEPLOYMENTS.items():
        content = (DEPLOY / name).read_text().replace("@CORTEX_USER@", cortex_user)
        target = system / name
        target.write_text(content)
        target.chmod(mode)
    shutil.copy2(DEPLOY / "manifest.json", system / "manifest.json")
    shutil.copy2(MANIFEST, system / "runtime-manifest.json")
    verify_staged(destination, manifest, cortex_user)


def verify_staged(destination, manifest=None, cortex_user=None):
    destination = Path(destination)
    cursor = destination
    while cursor != cursor.parent:
        if cursor.is_symlink():
            raise ValueError(f"staged artifact path contains a symlink: {cursor}")
        cursor = cursor.parent
    manifest = manifest or load_manifest()
    user_file = destination / "cortex-user.txt"
    if user_file.is_symlink() or not user_file.is_file():
        raise ValueError("staged Cortex service identity file is invalid")
    if cortex_user is None:
        cortex_user = user_file.read_text().strip()
    if not re.fullmatch(r"[a-z_][a-z0-9_-]*[$]?", cortex_user):
        raise ValueError("invalid staged Cortex service username")
    for directory_name in ("releases", "models", "system"):
        directory = destination / directory_name
        if directory.is_symlink() or not directory.is_dir():
            raise ValueError(f"staged Prism directory is invalid: {directory_name}")
    if {path.name for path in destination.iterdir()} != {"releases", "models", "system", "current", "cortex-user.txt"}:
        raise ValueError("staged Prism top-level inventory is invalid")
    release = destination / "releases" / manifest["release"]
    if release.is_symlink() or not release.is_dir():
        raise ValueError("staged Prism release directory is invalid")
    model_directory = destination / "models" / "bonsai"
    if model_directory.is_symlink() or not model_directory.is_dir():
        raise ValueError("staged Prism model directory is invalid")
    if not (destination / "current").is_symlink() or os.readlink(destination / "current") != f"releases/{manifest['release']}":
        raise ValueError("staged runtime current alias is invalid")
    check_runtime(release, manifest["entries"])
    check_models(model_directory, exact=True)
    expected_system = set(DEPLOYMENTS) | {"manifest.json", "runtime-manifest.json"}
    system_directory = destination / "system"
    if {p.name for p in system_directory.iterdir()} != expected_system:
        raise ValueError("staged Prism system-file inventory is invalid")
    for name, (_, mode) in DEPLOYMENTS.items():
        path = destination / "system" / name
        if path.is_symlink() or not path.is_file():
            raise ValueError(f"staged system file is invalid: {name}")
        expected = (DEPLOY / name).read_text().replace("@CORTEX_USER@", cortex_user)
        if path.read_text() != expected:
            raise ValueError(f"staged template mismatch: {name}")
        if stat.S_IMODE(path.stat().st_mode) != mode:
            raise ValueError(f"staged mode mismatch: {name}")
    for name, source in (("manifest.json", DEPLOY / "manifest.json"), ("runtime-manifest.json", MANIFEST)):
        target = destination / "system" / name
        if not target.is_file() or target.is_symlink() or digest(target) != digest(source):
            raise ValueError(f"staged Prism manifest mismatch: {name}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("action", choices=("stage", "check"))
    parser.add_argument("destination", type=Path)
    parser.add_argument("--runtime", type=Path)
    parser.add_argument("--models", type=Path)
    parser.add_argument("--cortex-user", default="")
    args = parser.parse_args()
    try:
        manifest = load_manifest()
        if args.action == "stage":
            if not args.runtime or not args.models or not args.cortex_user or "/" in args.cortex_user:
                raise ValueError("stage requires runtime, models, and a valid Cortex service username")
            stage(args.runtime, args.models, args.destination, args.cortex_user)
        else:
            verify_staged(args.destination, manifest)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"Prism staging verification failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
