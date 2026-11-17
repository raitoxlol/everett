"""Exercise the public shell installer with isolated release downloads."""
import hashlib
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest

INSTALLER = Path(__file__).resolve().parents[1] / 'install.sh'


class InstallTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.home = self.root / 'home'
        self.home.mkdir()
        self.tools = self.root / 'tools'
        self.tools.mkdir()
        self.destination = self.root / 'installed bin'
        self.destination.mkdir()
        self.installed = self.destination / 'everett'
        self.old = b'#!/bin/sh\necho "everett previous"\n'
        self.installed.write_bytes(self.old)
        self.installed.chmod(0o755)
        system = subprocess.check_output(['uname', '-s'], text=True).strip()
        machine = subprocess.check_output(['uname', '-m'], text=True).strip()
        targets = {'Linux-x86_64': 'x86_64-unknown-linux-musl', 'Linux-aarch64': 'aarch64-unknown-linux-musl', 'Linux-arm64': 'aarch64-unknown-linux-musl', 'Darwin-arm64': 'aarch64-apple-darwin', 'Darwin-x86_64': 'x86_64-apple-darwin'}
        self.name = 'everett-' + targets[system + '-' + machine]
        self.package = self.root / self.name
        self.package.mkdir()
        self.archive = self.root / (self.name + '.tar.gz')
        self.script(self.tools / 'curl', '#!/bin/sh\nset -eu\ncp "$INSTALL_TEST_RELEASE/${2##*/}" "$4"\n')

    def script(self, path, text):
        path.write_text(text)
        path.chmod(0o755)

    def release(self, body):
        self.script(self.package / 'everett', '#!/bin/sh\n' + body)
        with tarfile.open(self.archive, 'w:gz') as stream:
            stream.add(self.package, arcname=self.name)
        self.checksum = Path(str(self.archive) + '.sha256')
        self.checksum.write_text(hashlib.sha256(self.archive.read_bytes()).hexdigest() + '  ' + self.archive.name + '\n')

    def install(self):
        result = subprocess.run(['sh', str(INSTALLER)], env={'HOME': str(self.home), 'PATH': str(self.tools) + ':/usr/bin:/bin', 'EVERETT_VERSION': 'fixture', 'EVERETT_INSTALL_DIR': str(self.destination), 'INSTALL_TEST_RELEASE': str(self.root)}, capture_output=True, text=True, timeout=15)
        self.assertFalse(list(self.destination.glob('.everett-install.*')), 'staging files leaked')
        return result

    def rejected(self):
        result = self.install()
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertEqual(self.installed.read_bytes(), self.old)
        self.assertNotIn('installed ', result.stdout)

    def test_broken_candidate_preserves_installed_binary_and_fails(self):
        self.release('exit 7\n')
        self.rejected()

    def test_empty_or_wrong_version_preserves_installed_binary(self):
        for body in ['exit 0\n', 'echo "not Everett"\n']:
            with self.subTest(body=body):
                self.release(body)
                self.rejected()

    def test_checksum_failure_preserves_installed_binary(self):
        self.release('echo "everett 9.0.0"\n')
        self.checksum.write_text('0' * 64 + '  ' + self.archive.name + '\n')
        self.rejected()

    def test_failed_activation_preserves_installed_binary(self):
        self.release('echo "everett 9.0.0"\n')
        self.script(self.tools / 'mv', '#!/bin/sh\nexit 8\n')
        self.rejected()

    def test_fresh_install_and_repeated_upgrade(self):
        self.installed.unlink()
        self.release('echo "everett 9.0.0"\necho "revision: fixture"\n')
        for _ in range(2):
            result = self.install()
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn('installed everett 9.0.0', result.stdout)
            self.assertEqual(subprocess.check_output([str(self.installed), '--version'], text=True), 'everett 9.0.0\nrevision: fixture\n')

    def test_upgrade_keeps_running_previous_binary_alive(self):
        shutil.copyfile('/bin/sleep', self.installed)
        if sys.platform == 'darwin':
            # Copied Apple platform executables are killed outside their sealed path.
            subprocess.run(['codesign', '--force', '--sign', '-', str(self.installed)], check=True, capture_output=True)
        process = subprocess.Popen(['sleep', '20'], executable=str(self.installed))
        try:
            self.release('echo "everett 9.0.0"\n')
            result = self.install()
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIsNone(process.poll())
            self.assertEqual(subprocess.check_output([str(self.installed), '--version'], text=True), 'everett 9.0.0\n')
        finally:
            process.terminate()
            process.wait(timeout=5)


if __name__ == '__main__':
    unittest.main()
