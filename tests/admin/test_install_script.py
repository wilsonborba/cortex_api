"""Installer regression tests. All services, builds and system paths are doubles."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import tempfile
import unittest


class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name)
        self.repo = self.base / 'repo'
        self.repo.mkdir()
        source = Path(__file__).resolve().parents[2]
        self.script = 'install.sh' if (source / 'scripts/install.sh').exists() else 'install-service.sh'
        self.names = ['cortex-api'] if self.script == 'install.sh' else ['hippocampus-api', 'hippocampus']
        (self.repo / 'scripts').mkdir()
        shutil.copy2(source / 'scripts' / self.script, self.repo / 'scripts' / self.script)
        (self.repo / 'src').mkdir()
        (self.repo / 'src/main.rs').write_text('fn main() {}\n')
        (self.repo / 'Cargo.toml').write_text('[package]\nname="synthetic"\n')
        (self.repo / 'Cargo.lock').write_text('# synthetic\n')
        subprocess.run(['git', 'init', '-q', str(self.repo)], check=True)
        subprocess.run(['git', '-C', str(self.repo), 'add', '.'], check=True)
        self.build = self.base / 'external-target'
        (self.build / 'release').mkdir(parents=True)
        for name in self.names:
            p = self.build / 'release' / name
            p.write_bytes(Path('/bin/sleep').read_bytes() + name.encode())
            p.chmod(0o755)
        self.bin = self.base / 'bin'
        self.bin.mkdir()
        self.env = dict(os.environ, PATH=str(self.bin) + ':' + os.environ['PATH'],
                        TEST_REPO=str(self.repo), TEST_BUILD=str(self.build), TEST_ADMIN='1')
        self.helper('cargo', '''import json,os,sys
from pathlib import Path
if sys.argv[1]=='metadata': print(json.dumps({'target_directory':os.environ['TEST_BUILD']}))
elif os.environ.get('TEST_BUILD_FAIL')=='1': sys.exit(1)
elif os.environ.get('TEST_CHANGE_SOURCE')=='1': (Path(os.environ['TEST_REPO'])/'src/main.rs').write_text('changed during build')
''')
        self.helper('id', '''import os,sys
if sys.argv[1:]==['-u'] and os.environ.get('TEST_ADMIN')=='1': print('0')
else: os.execv('/usr/bin/id',['id']+sys.argv[1:])
''')
        self.helper('install', '''import os,sys
from pathlib import Path
args=sys.argv[1:]
if args[-1].startswith('/etc/systemd/system/'):
 p=Path(os.environ['TEST_REPO'])/'units'/Path(args[-1]).name;p.parent.mkdir(exist_ok=True);args[-1]=str(p)
os.execv('/usr/bin/install',['install']+args)
''')
        self.helper('systemctl', '''import json,os,subprocess,sys
from pathlib import Path
root=Path(os.environ['TEST_REPO']);args=sys.argv[1:];state=root/'pids.json'
pids=json.loads(state.read_text()) if state.exists() else {}
with (root/'service.log').open('a') as f:f.write(' '.join(args)+'\\n')
if args[0]=='restart':
 for unit in args[1:]:
  name='hippocampus' if unit=='hippocampus-scheduler.service' else unit.removesuffix('.service')
  p=subprocess.Popen([str(root/'target/release'/name),'60'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,start_new_session=True)
  pids[unit]=p.pid
 state.write_text(json.dumps(pids))
elif args[0]=='show':print(pids.get(args[1],0))
elif args[0]=='is-active':sys.exit(0 if args[-1] in pids else 1)
''')
        self.addCleanup(self.stop_doubles)

    def helper(self, name, body):
        p = self.bin / name
        p.write_text('#!/usr/bin/python3\n' + body)
        p.chmod(0o755)

    def stop_doubles(self):
        path = self.repo / 'pids.json'
        if path.exists():
            for pid in json.loads(path.read_text()).values():
                try:
                    os.kill(pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass

    def run_script(self, mode, success=True):
        r = subprocess.run(['bash', str(self.repo / 'scripts' / self.script), mode],
                           env=self.env, capture_output=True, text=True, timeout=15)
        if success:
            self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        else:
            self.assertNotEqual(r.returncode, 0)
            self.assertFalse((self.repo / 'service.log').exists(), 'Changed a service before validation')
        return r

    def test_external_target_is_staged_and_checked(self):
        self.run_script('--prepare')
        self.run_script('--check')
        self.assertFalse((self.repo / 'target').exists())
        for name in self.names:
            self.assertEqual((self.repo / 'var/deploy' / name).read_bytes(),
                             (self.build / 'release' / name).read_bytes())

    def test_activation_replaces_old_binaries_and_checks_running_artifacts(self):
        (self.repo / 'target/release').mkdir(parents=True)
        for name in self.names:
            (self.repo / 'target/release' / name).write_text('old executable')
        self.run_script('--prepare')
        result = self.run_script('--activate')
        self.assertIn('run the prepared release binaries', result.stdout)
        for name in self.names:
            self.assertEqual((self.repo / 'target/release' / name).read_bytes(),
                             (self.build / 'release' / name).read_bytes())
        self.assertEqual(len(list((self.repo / 'units').glob('*.service'))), len(self.names))

    def test_corrupted_binary_is_rejected_before_service_changes(self):
        self.run_script('--prepare')
        (self.repo / 'var/deploy' / self.names[0]).write_text('corrupt')
        self.run_script('--activate', success=False)

    def test_changed_source_is_rejected_before_service_changes(self):
        self.run_script('--prepare')
        (self.repo / 'src/main.rs').write_text('changed source')
        self.run_script('--activate', success=False)

    def test_missing_stage_is_rejected_before_service_changes(self):
        self.run_script('--activate', success=False)

    def test_failed_build_preserves_previous_stage(self):
        self.run_script('--prepare')
        before = (self.repo / 'var/deploy/artifacts.sha256').read_bytes()
        self.env['TEST_BUILD_FAIL'] = '1'
        self.run_script('--prepare', success=False)
        self.assertEqual(before, (self.repo / 'var/deploy/artifacts.sha256').read_bytes())

    def test_source_change_during_build_is_rejected(self):
        self.env['TEST_CHANGE_SOURCE'] = '1'
        self.run_script('--prepare', success=False)
        self.assertFalse((self.repo / 'var/deploy').exists())

    def test_checks_work_across_system_languages(self):
        for name in ['a_b.rs', 'ab.rs']:
            (self.repo / 'src' / name).write_text('// synthetic')
        subprocess.run(['git', '-C', str(self.repo), 'add', 'src'], check=True)
        self.env['LC_ALL'] = 'C.UTF-8'
        self.run_script('--prepare')
        self.env['LC_ALL'] = 'en_SG.UTF-8'
        self.run_script('--check')

    def test_changed_embedded_migration_is_rejected(self):
        (self.repo / 'migrations').mkdir()
        migration = self.repo / 'migrations/001.sql'
        migration.write_text('SELECT 1;')
        subprocess.run(['git', '-C', str(self.repo), 'add', 'migrations'], check=True)
        self.run_script('--prepare')
        migration.write_text('SELECT 2;')
        self.run_script('--activate', success=False)

    def test_activation_requires_administrator(self):
        self.run_script('--prepare')
        self.env['TEST_ADMIN'] = '0'
        if os.getuid() == 0:
            self.helper('id', "import sys\nif sys.argv[1:]==['-u']: print('1000')\nelse: print('users')\n")
        r = self.run_script('--activate', success=False)
        self.assertIn('Administrator rights', r.stderr)

    def test_units_from_another_checkout_are_rejected(self):
        self.run_script('--prepare')
        stage = self.repo / 'var/deploy'
        unit = next(stage.glob('*.service'))
        unit.write_text(unit.read_text().replace('WorkingDirectory=' + str(self.repo), 'WorkingDirectory=/synthetic/other'))
        files = [stage / n for n in self.names] + list(stage.glob('*.service'))
        (stage / 'artifacts.sha256').write_text(''.join(
            hashlib.sha256(p.read_bytes()).hexdigest() + '  ' + p.name + '\n' for p in files))
        self.run_script('--activate', success=False)


if __name__ == '__main__':
    unittest.main()
