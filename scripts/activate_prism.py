#!/usr/bin/env python3
"""Install the verified Prism host files without enabling or starting the service."""
import argparse
import importlib.util
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import uuid

ROOT = Path(__file__).resolve().parents[1]
STAGER_PATH = ROOT / "scripts/stage_prism.py"
SPEC = importlib.util.spec_from_file_location("stage_prism", STAGER_PATH)
stager = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(stager)


def parse_unit_status(output):
    fields = {}
    for line in output.splitlines():
        if "=" in line:
            key, value = line.split("=", 1)
            fields[key.strip()] = value.strip()
    required = {"LoadState", "ActiveState", "SubState", "MainPID"}
    if not required.issubset(fields):
        raise RuntimeError("systemd Prism status is incomplete")
    try:
        main_pid = int(fields["MainPID"])
    except ValueError as error:
        raise RuntimeError("systemd Prism MainPID is invalid") from error
    return fields, main_pid


def assert_prism_quiescent(run=subprocess.run):
    result = run([
        "systemctl", "--no-ask-password", "show", "prism.service",
        "--property=LoadState,ActiveState,SubState,MainPID,ControlGroup,InvocationID",
    ], check=False, capture_output=True, text=True)
    if result.returncode != 0:
        raise RuntimeError("could not query Prism service quiescence")
    fields, main_pid = parse_unit_status(result.stdout)
    inactive = fields["ActiveState"] in {"inactive", "failed"} or fields["LoadState"] == "not-found"
    if not inactive or main_pid != 0:
        raise RuntimeError(f"Prism service is not quiescent: {fields['ActiveState']}/{fields['SubState']} MainPID={main_pid}")


class RuntimePathSnapshot:
    """Reversible state for the two fixed tmpfiles-managed runtime paths."""

    def __init__(self, directory, selector):
        self.directory = Path(directory)
        self.selector = Path(selector)
        self.directory_state = self._capture(self.directory)
        self.selector_state = self._capture(self.selector)

    @staticmethod
    def _capture(path):
        if path.is_symlink():
            raise RuntimeError(f"tmpfiles-managed path cannot be a symlink: {path}")
        if not path.exists():
            return None
        info = path.lstat()
        if path.is_file():
            kind, content = "file", path.read_bytes()
        elif path.is_dir():
            kind, content = "directory", None
        else:
            raise RuntimeError(f"unsupported tmpfiles-managed path type: {path}")
        return kind, content, info.st_mode & 0o7777, info.st_uid, info.st_gid

    @staticmethod
    def _remove_if_created(path):
        if path.is_symlink() or (path.exists() and not path.is_dir()):
            path.unlink()
        elif path.is_dir():
            path.rmdir()

    @staticmethod
    def _restore(path, state):
        if state is None:
            RuntimePathSnapshot._remove_if_created(path)
            return
        kind, content, mode, uid, gid = state
        if kind == "directory":
            if path.is_symlink() or (path.exists() and not path.is_dir()):
                path.unlink()
            path.mkdir(parents=True, exist_ok=True)
            os.chmod(path, mode)
            os.chown(path, uid, gid)
            return
        if path.is_dir() and not path.is_symlink():
            path.rmdir()
        elif path.exists() or path.is_symlink():
            path.unlink()
        tmp = path.with_name(f".{path.name}.restore-{uuid.uuid4().hex}")
        try:
            tmp.write_bytes(content)
            os.chmod(tmp, mode)
            os.chown(tmp, uid, gid)
            os.replace(tmp, path)
        finally:
            if tmp.exists() or tmp.is_symlink():
                tmp.unlink()

    def restore(self):
        self._restore(self.selector, self.selector_state)
        self._restore(self.directory, self.directory_state)
class Transaction:
    def __init__(self, check_output=subprocess.check_output):
        self.backups = {}
        self.metadata = {}
        self.created_dirs = []
        self.temp_paths = []
        self.check_output = check_output

    def mkdir(self, path, mode, group=None):
        self.remember_metadata(path)
        missing = []
        current = path
        while not current.exists() and not current.is_symlink():
            missing.append(current)
            current = current.parent
        path.mkdir(parents=True, exist_ok=True)
        for item in reversed(missing):
            self.created_dirs.append(item)
        os.chmod(path, mode)
        if group is not None:
            gid = int(self.check_output(["getent", "group", group], text=True).split(":")[2])
            os.chown(path, 0, gid)

    def remember_metadata(self, path):
        if path.exists() and not path.is_symlink() and path not in self.metadata:
            old = path.stat()
            self.metadata[path] = (old.st_mode & 0o7777, old.st_uid, old.st_gid)

    def ensure_parent(self, path):
        missing = []
        current = path
        while not current.exists() and not current.is_symlink():
            missing.append(current)
            current = current.parent
        path.mkdir(parents=True, exist_ok=True)
        self.created_dirs.extend(reversed(missing))

    def backup(self, path):
        if path in self.backups:
            return
        if not path.exists() and not path.is_symlink():
            self.backups[path] = None
            return
        backup_root = Path(tempfile.mkdtemp(prefix="prism-rollback-"))
        self.temp_paths.append(backup_root)
        saved = backup_root / "saved"
        if path.is_symlink():
            saved.symlink_to(os.readlink(path))
        elif path.is_dir():
            shutil.copytree(path, saved, symlinks=True)
        else:
            shutil.copy2(path, saved)
        old = path.lstat()
        self.backups[path] = (saved, old.st_mode & 0o7777, old.st_uid, old.st_gid)

    @staticmethod
    def temporary_sibling(path):
        return path.with_name(f".{path.name}.prism-new-{uuid.uuid4().hex}")

    def replace_file(self, source, destination, mode, group=None):
        self.ensure_parent(destination.parent)
        self.backup(destination)
        tmp = self.temporary_sibling(destination)
        try:
            shutil.copy2(source, tmp)
            os.chmod(tmp, mode)
            if group is not None:
                gid = int(self.check_output(["getent", "group", group], text=True).split(":")[2])
                os.chown(tmp, 0, gid)
            os.replace(tmp, destination)
        finally:
            if tmp.exists() or tmp.is_symlink():
                tmp.unlink()

    def replace_link(self, target, destination):
        self.ensure_parent(destination.parent)
        self.backup(destination)
        tmp = self.temporary_sibling(destination)
        try:
            tmp.symlink_to(target)
            os.replace(tmp, destination)
        finally:
            if tmp.exists() or tmp.is_symlink():
                tmp.unlink()

    def rollback(self):
        for path, saved_state in reversed(list(self.backups.items())):
            if path.is_dir() and not path.is_symlink():
                shutil.rmtree(path)
            elif path.exists() or path.is_symlink():
                path.unlink()
            if saved_state is not None:
                saved, mode, uid, gid = saved_state
                path.parent.mkdir(parents=True, exist_ok=True)
                if saved.is_dir() and not saved.is_symlink():
                    shutil.copytree(saved, path, symlinks=True)
                elif saved.is_symlink():
                    path.symlink_to(os.readlink(saved))
                else:
                    shutil.copy2(saved, path)
                if not path.is_symlink():
                    os.chmod(path, mode)
                    os.chown(path, uid, gid)
        for path, (mode, uid, gid) in self.metadata.items():
            if path.exists() and not path.is_symlink():
                os.chmod(path, mode)
                os.chown(path, uid, gid)
        for path in reversed(self.created_dirs):
            try:
                path.rmdir()
            except OSError:
                pass
        for path in self.temp_paths:
            shutil.rmtree(path, ignore_errors=True)

    def commit(self):
        for path in self.temp_paths:
            shutil.rmtree(path, ignore_errors=True)


def ensure_prism_account(run=subprocess.run):
    created_group = False
    created_user = False
    groups = run(["getent", "group", "prism"], check=False, capture_output=True, text=True)
    if groups.returncode:
        run(["groupadd", "--system", "prism"], check=True)
        created_group = True
    try:
        users = run(["getent", "passwd", "prism"], check=False, capture_output=True, text=True)
        if users.returncode:
            run(["useradd", "--system", "--gid", "prism", "--home-dir", "/nonexistent", "--shell", "/usr/sbin/nologin", "prism"], check=True)
            created_user = True
        else:
            fields = users.stdout.strip().split(":")
            groups = groups.stdout.strip().split(":") if groups.stdout else []
            if (len(fields) < 7 or fields[0] != "prism" or fields[6] != "/usr/sbin/nologin"
                    or fields[5] != "/nonexistent" or len(groups) < 3 or fields[3] != groups[2]):
                raise RuntimeError("existing prism account does not match the dedicated service account contract")
    except Exception:
        if created_group:
            run(["groupdel", "prism"], check=False)
        raise
    return created_group, created_user


def assert_safe_destination(path, filesystem_root, allow_leaf_symlink=False):
    if filesystem_root.is_symlink():
        raise RuntimeError("activation filesystem root cannot be a symlink")
    relative = path.relative_to(filesystem_root)
    cursor = filesystem_root
    for component in relative.parts[:-1]:
        cursor = cursor / component
        if cursor.is_symlink():
            raise RuntimeError(f"activation path contains a symlinked parent: {cursor}")
    if path.is_symlink() and not allow_leaf_symlink:
        raise RuntimeError(f"activation destination cannot be a symlink: {path}")


def activate(staged, filesystem_root=Path("/"), run=subprocess.run, check_output=subprocess.check_output):
    stager.verify_staged(staged)
    staged = staged.resolve(strict=True)
    if os.geteuid() != 0 and filesystem_root == Path("/"):
        raise PermissionError("Prism activation requires administrator rights")
    assert_prism_quiescent(run)
    tx = Transaction(check_output)
    host = lambda path: filesystem_root / path.lstrip("/")
    stage_manifest = stager.load_manifest()
    release_name = stage_manifest["release"]
    release_src = staged / "releases" / release_name
    release_dst = host(f"/opt/prism/releases/{release_name}")
    models_src = staged / "models/bonsai"
    system_src = staged / "system"
    destinations = [
        (host("/opt/prism/releases"), False), (release_dst, False),
        (host("/opt/prism/current"), True), (host("/var/lib/prism/models/bonsai"), False),
        (host("/etc/prism"), False),
        (host("/etc/prism/manifest.json"), False), (host("/etc/prism/runtime-manifest.json"), False),
        (host("/etc/systemd/system/prism.service"), False),
        (host("/usr/local/bin/prism-launch"), False),
        (host("/etc/tmpfiles.d/prism.conf"), False),
        (host("/etc/polkit-1/rules.d/49-prism-systemd.rules"), False),
        (host("/run/prism"), False), (host("/run/prism/profile"), False),
        (host("/var/lib/prism/models/bonsai") / next(iter(stager.MODELS)), False),
        (host("/var/lib/prism/models/bonsai") / list(stager.MODELS)[1], False),
    ]
    for destination, allow_symlink in destinations:
        assert_safe_destination(destination, filesystem_root, allow_symlink)
    runtime_paths = RuntimePathSnapshot(host("/run/prism"), host("/run/prism/profile"))
    created_group, created_user = ensure_prism_account(run)
    try:
        for directory, mode, group in ((host("/opt/prism/releases"), 0o755, "prism"), (host("/var/lib/prism/models/bonsai"), 0o750, "prism"), (host("/etc/prism"), 0o750, "prism")):
            tx.mkdir(directory, mode, group)
        if release_dst.exists():
            stager.check_runtime(release_dst, stage_manifest["entries"])
        else:
            tx.backup(release_dst)
            tmp = tx.temporary_sibling(release_dst)
            try:
                shutil.copytree(release_src, tmp, symlinks=True)
                os.chmod(tmp, 0o755)
                gid = int(check_output(["getent", "group", "prism"], text=True).split(":")[2])
                for path in tmp.iterdir():
                    if path.is_symlink():
                        continue
                    os.chown(path, 0, gid)
                    os.chmod(path, 0o750 if path.name == "llama-server" else 0o640)
                os.chown(tmp, 0, gid)
                os.replace(tmp, release_dst)
            finally:
                if tmp.exists() or tmp.is_symlink():
                    shutil.rmtree(tmp, ignore_errors=True)
        for name, (_, expected_sha) in stager.MODELS.items():
            source = models_src / name
            target = host(f"/var/lib/prism/models/bonsai/{name}")
            if target.exists():
                if target.is_symlink() or stager.digest(target) != expected_sha:
                    raise RuntimeError(f"existing Prism model conflicts with pinned asset: {name}")
                tx.remember_metadata(target)
                gid = int(check_output(["getent", "group", "prism"], text=True).split(":")[2])
                os.chmod(target, 0o640)
                os.chown(target, 0, gid)
                continue
            tx.backup(target)
            tmp = tx.temporary_sibling(target)
            try:
                shutil.copy2(source, tmp)
                os.chmod(tmp, 0o640)
                gid = int(check_output(["getent", "group", "prism"], text=True).split(":")[2])
                os.chown(tmp, 0, gid)
                os.replace(tmp, target)
            finally:
                if tmp.exists() or tmp.is_symlink():
                    tmp.unlink()
        for name in ("manifest.json", "runtime-manifest.json"):
            tx.replace_file(system_src / name, host(f"/etc/prism/{name}"), 0o640, "prism")
        for name, (target, mode) in stager.DEPLOYMENTS.items():
            tx.replace_file(system_src / name, host(target), mode)
        tx.replace_link(f"releases/{release_name}", host("/opt/prism/current"))
        run(["systemctl", "daemon-reload"], check=True)
        # Create only the runtime profile directory and selector; never start or enable Prism.
        run(["systemd-tmpfiles", "--create", str(host("/etc/tmpfiles.d/prism.conf"))], check=True)
        tx.commit()
    except Exception:
        rollback_errors = []
        for rollback in (tx.rollback, runtime_paths.restore):
            try:
                rollback()
            except Exception as rollback_error:
                rollback_errors.append(str(rollback_error))
        run(["systemctl", "daemon-reload"], check=False)
        if created_user:
            run(["userdel", "prism"], check=False)
        if created_group:
            run(["groupdel", "prism"], check=False)
        if rollback_errors:
            raise RuntimeError("activation rollback was incomplete: " + "; ".join(rollback_errors))
        raise


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--preflight", action="store_true")
    parser.add_argument("staged", type=Path, nargs="?")
    args = parser.parse_args()
    try:
        if args.preflight:
            if args.staged is not None:
                parser.error("--preflight does not accept a staged directory")
            assert_prism_quiescent()
            print("Prism service is quiescent (inactive/failed/not-found with MainPID=0).")
            return 0
        if args.staged is None:
            parser.error("a verified staged Prism directory is required")
        activate(args.staged)
    except Exception as error:
        print(f"Prism activation failed and installed files were rolled back: {error}", file=sys.stderr)
        return 1
    print("Prism files installed and verified. prism.service remains disabled and stopped.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
