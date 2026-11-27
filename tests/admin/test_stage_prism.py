"""Focused, filesystem-only tests for the Prism staging verifier."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
import unittest.mock

SCRIPT = Path(__file__).resolve().parents[2] / "scripts/stage_prism.py"
SPEC = importlib.util.spec_from_file_location("stage_prism", SCRIPT)
stager = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(stager)


def write_contract(deploy, entries):
    contract = {
        "schema_version": "1.0.0",
        "service": {"unit_name": "prism.service", "loopback_address": "127.0.0.1", "fixed_port": 49100, "cortex_binding": "cortex-api.service"},
        "profiles": {name: {"model_path": values[0], "model_sha256": values[1], "runtime_alias": values[2]} for name, values in stager.PROFILE_BINDINGS.items()},
        "runtime_server": {"path": "/opt/prism/current/llama-server", "sha256": next(item["sha256"] for item in entries if item["path"] == "llama-server")},
        "staged_files": [],
    }
    for name, (target, mode) in stager.DEPLOYMENTS.items():
        path = deploy / name
        contract["staged_files"].append({"path": f"deploy/prism/{name}", "target_path": target, "mode": f"{mode:04o}", "sha256": hashlib.sha256(path.read_bytes()).hexdigest()})
    (deploy / "manifest.json").write_text(json.dumps(contract))


class PrismStagingTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.runtime = self.root / "runtime"
        self.runtime.mkdir()
        (self.runtime / "llama-server").write_bytes(b"pinned server")
        (self.runtime / "llama-server").chmod(0o755)
        (self.runtime / "libllama.so.1").write_bytes(b"library")
        (self.runtime / "libllama.so").symlink_to("libllama.so.1")
        self.models = self.root / "models"
        self.models.mkdir()
        (self.models / "uncensored.gguf").write_bytes(b"model one")
        (self.models / "official.gguf").write_bytes(b"model two")
        self.deploy = self.root / "deploy"
        self.deploy.mkdir()
        stager.DEPLOYMENTS = {
            "prism.service": ("/etc/systemd/system/prism.service", 0o644),
            "prism-launch": ("/usr/local/bin/prism-launch", 0o755),
            "prism-tmpfiles.conf": ("/etc/tmpfiles.d/prism.conf", 0o644),
            "49-prism-systemd.rules": ("/etc/polkit-1/rules.d/49-prism-systemd.rules", 0o644),
        }
        (self.deploy / "prism.service").write_text("User=prism\nGroup=prism\nBindsTo=cortex-api.service\nAfter=cortex-api.service\nRestart=no\nKillMode=control-group\nTimeoutStopSec=30\nNoNewPrivileges=yes\n@CORTEX_USER@\n")
        (self.deploy / "prism-launch").write_text("unset LLAMA_API_KEY\nunset LLAMA_ARG_API_KEY_FILE\n--host 127.0.0.1\n")
        (self.deploy / "prism-tmpfiles.conf").write_text("d /run/prism 2750 @CORTEX_USER@ prism - -\n")
        (self.deploy / "49-prism-systemd.rules").write_text('unit === "prism.service"; verb === "start"; verb === "stop";\n')
        (self.deploy / "runtime-manifest.json").write_text("{}\n")
        stager.DEPLOY = self.deploy
        stager.MODELS = {
            "uncensored.gguf": (9, hashlib.sha256(b"model one").hexdigest()),
            "official.gguf": (9, hashlib.sha256(b"model two").hexdigest()),
        }
        entries = []
        for name, target in [("llama-server", None), ("libllama.so.1", None), ("libllama.so", "libllama.so.1")]:
            content = (self.runtime / (target or name)).read_bytes()
            entries.append({"path": name, "bytes": len(content), "sha256": hashlib.sha256(content).hexdigest(), "symlink": target is not None, "target": target})
        write_contract(self.deploy, entries)
        self.manifest = self.root / "runtime-manifest.json"
        self.manifest.write_text(json.dumps({"schema_version": "1.0.0", "release": stager.RUNTIME_RELEASE, "upstream_commit_prefix": stager.RUNTIME_UPSTREAM_COMMIT_PREFIX, "archive_sha256": stager.RUNTIME_ARCHIVE_SHA256, "entries": entries}))
        stager.MANIFEST = self.manifest
        stager.RUNTIME_INVENTORY_SHA256 = hashlib.sha256(self.manifest.read_bytes()).hexdigest()
        stager.RUNTIME_SERVER_SHA256 = next(item["sha256"] for item in entries if item["path"] == "llama-server")
        self.stage = self.root / "staged"

    def test_stage_and_check_pinned_inventory_and_fixed_templates(self):
        stager.stage(self.runtime, self.models, self.stage, "cortex")
        stager.verify_staged(self.stage)
        self.assertEqual(os.readlink(self.stage / "current"), f"releases/{stager.RUNTIME_RELEASE}")
        self.assertIn("User=prism\n", (self.stage / "system/prism.service").read_text())
        self.assertTrue((self.stage / "system/prism.service").read_text().endswith("cortex\n"))
        self.assertTrue((self.stage / "releases" / stager.RUNTIME_RELEASE / "libllama.so").is_symlink())

    def test_runtime_hash_mismatch_fails_closed(self):
        (self.runtime / "libllama.so.1").write_bytes(b"tampered")
        with self.assertRaisesRegex(ValueError, "runtime file hash"):
            stager.check_runtime(self.runtime, stager.load_manifest()["entries"])

    def test_runtime_symlink_escape_fails_closed(self):
        (self.runtime / "libllama.so").unlink()
        (self.runtime / "libllama.so").symlink_to("../outside")
        with self.assertRaisesRegex(ValueError, "symlink target mismatch"):
            stager.check_runtime(self.runtime, stager.load_manifest()["entries"])

    def test_missing_model_or_extra_profile_fails_closed(self):
        (self.models / "official.gguf").unlink()
        with self.assertRaisesRegex(ValueError, "model inventory"):
            stager.check_models(self.models)

    def test_asset_source_may_retain_non_profile_downloads(self):
        (self.models / "verified-runtime.tar.gz").write_bytes(b"archive retained separately")
        stager.check_models(self.models)
        with self.assertRaisesRegex(ValueError, "model inventory"):
            stager.check_models(self.models, exact=True)

    def test_staged_file_tampering_fails_check(self):
        stager.stage(self.runtime, self.models, self.stage, "cortex")
        (self.stage / "system/prism-launch").write_text("arbitrary command\n")
        with self.assertRaisesRegex(ValueError, "template mismatch"):
            stager.verify_staged(self.stage)

    def test_staged_release_directory_symlink_fails_closed(self):
        stager.stage(self.runtime, self.models, self.stage, "cortex")
        release = self.stage / "releases" / stager.RUNTIME_RELEASE
        outside = self.root / "outside-release"
        release.rename(outside)
        release.symlink_to(outside, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, "release directory is invalid"):
            stager.verify_staged(self.stage)

    def test_unapproved_profile_binding_fails_closed(self):
        contract = json.loads((self.deploy / "manifest.json").read_text())
        contract["profiles"]["bonsai25-official"]["model_path"] = "/tmp/other.gguf"
        (self.deploy / "manifest.json").write_text(json.dumps(contract))
        with self.assertRaisesRegex(ValueError, "profile binding mismatch"):
            stager.load_manifest()

    def test_deployment_source_pin_mismatch_fails_closed(self):
        with (self.deploy / "prism-launch").open("a") as stream:
            stream.write("arbitrary shell command\n")
        with self.assertRaisesRegex(ValueError, "deployment source pin mismatch"):
            stager.load_manifest()


# The activation helper receives filesystem and command doubles in these tests;
# production CLI has no host-root override and always targets the real host.
ACTIVATE_SCRIPT = Path(__file__).resolve().parents[2] / "scripts/activate_prism.py"
ACTIVATE_SPEC = importlib.util.spec_from_file_location("activate_prism", ACTIVATE_SCRIPT)
activator = importlib.util.module_from_spec(ACTIVATE_SPEC)
ACTIVATE_SPEC.loader.exec_module(activator)
activator.stager = stager


class PrismActivationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.runtime = self.root / "runtime"
        self.runtime.mkdir()
        (self.runtime / "llama-server").write_bytes(b"pinned server")
        (self.runtime / "llama-server").chmod(0o755)
        (self.runtime / "libllama.so.1").write_bytes(b"library")
        (self.runtime / "libllama.so").symlink_to("libllama.so.1")
        self.models = self.root / "models"
        self.models.mkdir()
        (self.models / "uncensored.gguf").write_bytes(b"model one")
        (self.models / "official.gguf").write_bytes(b"model two")
        self.deploy = self.root / "deploy"
        self.deploy.mkdir()
        stager.DEPLOYMENTS = {
            "prism.service": ("/etc/systemd/system/prism.service", 0o644),
            "prism-launch": ("/usr/local/bin/prism-launch", 0o755),
            "prism-tmpfiles.conf": ("/etc/tmpfiles.d/prism.conf", 0o644),
            "49-prism-systemd.rules": ("/etc/polkit-1/rules.d/49-prism-systemd.rules", 0o644),
        }
        (self.deploy / "prism.service").write_text("User=prism\nGroup=prism\nBindsTo=cortex-api.service\nAfter=cortex-api.service\nRestart=no\nKillMode=control-group\nTimeoutStopSec=30\nNoNewPrivileges=yes\n@CORTEX_USER@\n")
        (self.deploy / "prism-launch").write_text("unset LLAMA_API_KEY\nunset LLAMA_ARG_API_KEY_FILE\n--host 127.0.0.1\n")
        (self.deploy / "prism-tmpfiles.conf").write_text("d /run/prism 2750 @CORTEX_USER@ prism - -\n")
        (self.deploy / "49-prism-systemd.rules").write_text('unit === "prism.service"; verb === "start"; verb === "stop";\n')
        stager.DEPLOY = self.deploy
        stager.MODELS = {
            "uncensored.gguf": (9, hashlib.sha256(b"model one").hexdigest()),
            "official.gguf": (9, hashlib.sha256(b"model two").hexdigest()),
        }
        entries = []
        for name, target in [("llama-server", None), ("libllama.so.1", None), ("libllama.so", "libllama.so.1")]:
            content = (self.runtime / (target or name)).read_bytes()
            entries.append({"path": name, "bytes": len(content), "sha256": hashlib.sha256(content).hexdigest(), "symlink": target is not None, "target": target})
        write_contract(self.deploy, entries)
        self.manifest = self.root / "runtime-manifest.json"
        self.manifest.write_text(json.dumps({"schema_version": "1.0.0", "release": stager.RUNTIME_RELEASE, "upstream_commit_prefix": stager.RUNTIME_UPSTREAM_COMMIT_PREFIX, "archive_sha256": stager.RUNTIME_ARCHIVE_SHA256, "entries": entries}))
        stager.MANIFEST = self.manifest
        stager.RUNTIME_INVENTORY_SHA256 = hashlib.sha256(self.manifest.read_bytes()).hexdigest()
        stager.RUNTIME_SERVER_SHA256 = next(item["sha256"] for item in entries if item["path"] == "llama-server")
        activator.stager = stager
        self.stage = self.root / "staged"
        stager.stage(self.runtime, self.models, self.stage, "cortex")
        self.host = self.root / "host"
        self.commands = []

    def command_runner(self, fail_on=None):
        def run(args, check=False, **kwargs):
            self.commands.append(list(args))
            output = "prism:x:123:123:Prism:/nonexistent:/usr/sbin/nologin\n" if args[:2] == ["getent", "passwd"] else "prism:x:123\n"
            rc = 3 if args[:3] == ["systemctl", "is-active", "--quiet"] else (1 if fail_on == args[0] else 0)
            result = subprocess.CompletedProcess(args, rc,
                                                 stdout=output, stderr="")
            if check and result.returncode:
                raise subprocess.CalledProcessError(result.returncode, args)
            return result
        return run

    def group_output(self, args, text=False):
        return "prism:x:123\n" if text else b"prism:x:123\n"

    def test_activation_installs_fixed_files_without_starting_prism_and_is_repeatable(self):
        with unittest.mock.patch.object(activator.os, "chown"):
            activator.activate(self.stage, self.host, self.command_runner(), self.group_output)
            activator.activate(self.stage, self.host, self.command_runner(), self.group_output)
        self.assertEqual(os.readlink(self.host / "opt/prism/current"), f"releases/{stager.RUNTIME_RELEASE}")
        self.assertEqual((self.host / "var/lib/prism/models/bonsai/uncensored.gguf").read_bytes(), b"model one")
        self.assertTrue((self.host / "etc/systemd/system/prism.service").is_file())
        flat = [arg for command in self.commands for arg in command]
        self.assertNotIn("start", flat)
        self.assertNotIn("enable", flat)
        self.assertTrue(all(command[:3] == ["systemctl", "is-active", "--quiet"] or "prism.service" not in command for command in self.commands))

    def test_failed_tmpfiles_activation_restores_previous_alias_and_files(self):
        (self.host / "opt/prism/releases").mkdir(parents=True)
        (self.host / "opt/prism/releases/previous").mkdir()
        (self.host / "opt/prism/current").symlink_to("releases/previous")
        target = self.host / "etc/systemd/system/prism.service"
        target.parent.mkdir(parents=True)
        target.write_text("previous unit\n")
        with unittest.mock.patch.object(activator.os, "chown"):
            with self.assertRaises(subprocess.CalledProcessError):
                activator.activate(self.stage, self.host, self.command_runner("systemd-tmpfiles"), self.group_output)
        self.assertEqual(os.readlink(self.host / "opt/prism/current"), "releases/previous")
        self.assertEqual(target.read_text(), "previous unit\n")
        self.assertFalse((self.host / "opt/prism/releases" / stager.RUNTIME_RELEASE).exists())
        self.assertFalse((self.host / "var/lib/prism/models/bonsai/uncensored.gguf").exists())
        self.assertTrue(any(command[:2] == ["systemctl", "daemon-reload"] for command in self.commands))

    def test_symlinked_host_parent_fails_before_account_or_file_changes(self):
        self.host.mkdir()
        outside = self.root / "outside"
        outside.mkdir()
        (self.host / "opt").mkdir()
        (self.host / "opt/prism").symlink_to(outside, target_is_directory=True)
        with self.assertRaisesRegex(RuntimeError, "symlinked parent"):
            activator.activate(self.stage, self.host, self.command_runner(), self.group_output)
        self.assertEqual([command[1:3] for command in self.commands], [["is-active", "--quiet"]])
        self.assertEqual(list(outside.iterdir()), [])

    def test_new_service_account_is_removed_when_activation_rolls_back(self):
        def missing_account_runner(args, check=False, **kwargs):
            self.commands.append(list(args))
            if args[:2] in (["getent", "group"], ["getent", "passwd"]):
                result = subprocess.CompletedProcess(args, 1, stdout="", stderr="not found")
            elif args[:3] == ["systemctl", "is-active", "--quiet"]:
                result = subprocess.CompletedProcess(args, 3, stdout="", stderr="")
            elif args[0] == "systemd-tmpfiles":
                result = subprocess.CompletedProcess(args, 1, stdout="", stderr="simulated failure")
            else:
                result = subprocess.CompletedProcess(args, 0, stdout="", stderr="")
            if check and result.returncode:
                raise subprocess.CalledProcessError(result.returncode, args)
            return result

        with unittest.mock.patch.object(activator.os, "chown"):
            with self.assertRaises(subprocess.CalledProcessError):
                activator.activate(self.stage, self.host, missing_account_runner, self.group_output)
        self.assertIn(["userdel", "prism"], self.commands)
        self.assertIn(["groupdel", "prism"], self.commands)
        self.assertFalse((self.host / "opt/prism/current").exists())


if __name__ == "__main__":
    unittest.main()
