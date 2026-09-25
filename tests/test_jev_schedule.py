import sandbox  # noqa: F401  (must be first: isolates HOME)

import argparse
import io
import os
import stat
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest import mock

from everett import config, onboard, trunk_schedule
from everett.cli import main
from everett.route import RouteError, verify_key
from everett.session import home


class IsolatedHome(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self._patch = mock.patch.dict(os.environ, {'HOME': self._tmp.name, 'EVERETT_HOME': self._tmp.name})
        self._patch.start()
        self.addCleanup(self._patch.stop)


# ---- config.set_value -------------------------------------------------------------------------

class ConfigSetValue(IsolatedHome):
    def test_creates_file_chmod_600(self):
        path = config.set_value('typesafe_api_key', 'sk-test-123')
        self.assertTrue(path.exists())
        mode = stat.S_IMODE(path.stat().st_mode)
        self.assertEqual(mode, 0o600)
        self.assertEqual(config.get('typesafe_api_key'), 'sk-test-123')

    def test_preserves_other_top_level_keys_and_sections(self):
        config.config_path().parent.mkdir(parents=True, exist_ok=True)
        config.config_path().write_text(
            'vault = "~/Notes"\nrouter = "local"\n\n[jev]\napi_key = "old"\n', encoding='utf-8')
        config.set_value('typesafe_api_key', 'sk-new')
        text = config.config_path().read_text(encoding='utf-8')
        self.assertIn('vault = "~/Notes"', text)
        self.assertIn('router = "local"', text)
        self.assertIn('[jev]', text)
        self.assertIn('api_key = "old"', text)
        self.assertIn('typesafe_api_key = "sk-new"', text)
        # note: [jev].api_key aliases to the same key and, declared later in the file, wins on
        # read -- that's existing config.py precedence, not something set_value changes.

    def test_overwrites_existing_key_in_place_idempotent(self):
        config.set_value('typesafe_api_key', 'sk-one')
        config.set_value('typesafe_api_key', 'sk-two')
        text = config.config_path().read_text(encoding='utf-8')
        self.assertEqual(text.count('typesafe_api_key'), 1)
        self.assertEqual(config.get('typesafe_api_key'), 'sk-two')

    def test_never_writes_inside_a_section(self):
        config.config_path().parent.mkdir(parents=True, exist_ok=True)
        config.config_path().write_text('[jev]\napi_key = "old"\n', encoding='utf-8')
        config.set_value('typesafe_api_key', 'sk-top')
        text = config.config_path().read_text(encoding='utf-8')
        # the new top-level key must land before the [jev] section, not inside it
        self.assertLess(text.index('typesafe_api_key'), text.index('[jev]'))


# ---- route.verify_key --------------------------------------------------------------------------

class VerifyKey(unittest.TestCase):
    def test_ok_when_jev_succeeds(self):
        self.assertTrue(verify_key('sk-key', jev=lambda *a: {'choice': 'new', 'confidence': 0.9}))

    def test_false_when_jev_raises_route_error(self):
        def boom(*a):
            raise RouteError(2, 'nope')
        self.assertFalse(verify_key('sk-key', jev=boom))


# ---- onboard: find_jev_key_with_source ----------------------------------------------------------

class FindJevKeySource(IsolatedHome):
    def test_none_found(self):
        self.assertEqual(onboard.find_jev_key_with_source(), ('', ''))

    def test_env_wins(self):
        with mock.patch.dict(os.environ, {'TYPESAFE_API_KEY': 'from-env'}):
            self.assertEqual(onboard.find_jev_key_with_source(), ('from-env', 'env'))

    def test_config_when_no_env(self):
        config.set_value('typesafe_api_key', 'from-config')
        self.assertEqual(onboard.find_jev_key_with_source(), ('from-config', 'config'))

    def test_hermes_env_file_fallback(self):
        hermes = home() / '.hermes'
        hermes.mkdir(parents=True, exist_ok=True)
        (hermes / '.env').write_text('OTHER=1\nTYPESAFE_API_KEY=from-hermes\n', encoding='utf-8')
        self.assertEqual(onboard.find_jev_key_with_source(), ('from-hermes', 'hermes'))

    def test_env_beats_config_and_hermes(self):
        config.set_value('typesafe_api_key', 'from-config')
        hermes = home() / '.hermes'
        hermes.mkdir(parents=True, exist_ok=True)
        (hermes / '.env').write_text('TYPESAFE_API_KEY=from-hermes\n', encoding='utf-8')
        with mock.patch.dict(os.environ, {'TYPESAFE_API_KEY': 'from-env'}):
            self.assertEqual(onboard.find_jev_key_with_source(), ('from-env', 'env'))


# ---- onboard: apply_jev --------------------------------------------------------------------------

class ApplyJev(IsolatedHome):
    def test_skip_writes_nothing(self):
        cfg = onboard.OnboardConfig(jev_choice='skip')
        line = onboard.apply_jev(cfg)
        self.assertIn('skipped', line)
        self.assertFalse(config.config_path().exists())

    def test_paste_saves_key_masked_report(self):
        cfg = onboard.OnboardConfig(jev_choice='paste', jev_key='sk-secret-value', jev_validated=True)
        line = onboard.apply_jev(cfg)
        self.assertIn('validated', line)
        self.assertNotIn('sk-secret-value', line)  # the key itself is never echoed in the report
        self.assertEqual(config.get('typesafe_api_key'), 'sk-secret-value')
        mode = stat.S_IMODE(config.config_path().stat().st_mode)
        self.assertEqual(mode, 0o600)

    def test_paste_failed_validation_kept_anyway_reported(self):
        cfg = onboard.OnboardConfig(jev_choice='paste', jev_key='sk-bad', jev_validated=False)
        line = onboard.apply_jev(cfg)
        self.assertIn('validation failed', line)
        self.assertEqual(config.get('typesafe_api_key'), 'sk-bad')


class ApplyJevKeyEnvYesMode(IsolatedHome):
    def test_uses_found_key_ignores_env_flag(self):
        with mock.patch.dict(os.environ, {'TYPESAFE_API_KEY': 'already-here'}):
            cfg = onboard.OnboardConfig()
            onboard.apply_jev_key_env(cfg, 'SOME_VAR')
            self.assertEqual(cfg.jev_choice, 'skip')
            self.assertEqual(cfg.jev_found_source, 'env')

    def test_reads_named_env_var_and_validates(self):
        with mock.patch.dict(os.environ, {'MY_JEV_KEY': 'sk-yes'}), \
             mock.patch('everett.onboard.verify_key', return_value=True) as vk:
            cfg = onboard.OnboardConfig()
            onboard.apply_jev_key_env(cfg, 'MY_JEV_KEY')
            vk.assert_called_once_with('sk-yes')
            self.assertEqual(cfg.jev_choice, 'paste')
            self.assertEqual(cfg.jev_key, 'sk-yes')
            self.assertTrue(cfg.jev_validated)

    def test_no_var_given_or_unset_skips(self):
        cfg = onboard.OnboardConfig()
        onboard.apply_jev_key_env(cfg, None)
        self.assertEqual(cfg.jev_choice, 'skip')
        cfg2 = onboard.OnboardConfig()
        onboard.apply_jev_key_env(cfg2, 'UNSET_VAR_XYZ')
        self.assertEqual(cfg2.jev_choice, 'skip')


# ---- onboard --yes end-to-end: jev + trunk schedule -----------------------------------------------

class OnboardYesJevAndSchedule(IsolatedHome):
    def test_yes_jev_key_env_saved_and_scheduled(self):
        with mock.patch.dict(os.environ, {'MY_KEY': 'sk-from-yes'}), \
             mock.patch('everett.onboard.verify_key', return_value=True), \
             mock.patch('everett.trunk_schedule.subprocess.run') as run:
            run.return_value = mock.Mock(returncode=0, stderr='')
            out = io.StringIO()
            with redirect_stdout(out):
                rc = main(['onboard', '--yes', '--jev-key-env', 'MY_KEY', '--schedule-merge'])
        self.assertEqual(rc, 0)
        self.assertEqual(config.get('typesafe_api_key'), 'sk-from-yes')
        self.assertTrue(trunk_schedule.plist_path().exists())
        text = out.getvalue()
        self.assertIn('Smarter routing:', text)
        self.assertIn('Nightly merge:', text)
        launchctl_calls = [c for c in run.call_args_list if c.args[0][:2] == ['launchctl', 'bootstrap']]
        self.assertEqual(len(launchctl_calls), 1)

    def test_yes_default_skips_jev_and_schedule(self):
        out = io.StringIO()
        with redirect_stdout(out):
            rc = main(['onboard', '--yes'])
        self.assertEqual(rc, 0)
        self.assertFalse(config.config_path().exists())
        self.assertFalse(trunk_schedule.plist_path().exists())


# ---- onboard plain-prompt: jev entry ---------------------------------------------------------------

class PlainJevEntry(IsolatedHome):
    def test_getpass_key_validated_and_saved(self):
        cfg = onboard.OnboardConfig()
        with mock.patch('getpass.getpass', return_value='sk-typed'), \
             mock.patch('everett.onboard.verify_key', return_value=True):
            out = io.StringIO()
            with redirect_stdout(out):
                onboard._plain_jev_entry(cfg)
        self.assertEqual(cfg.jev_choice, 'paste')
        self.assertEqual(cfg.jev_key, 'sk-typed')
        self.assertTrue(cfg.jev_validated)

    def test_getpass_empty_skips(self):
        cfg = onboard.OnboardConfig()
        with mock.patch('getpass.getpass', return_value=''):
            onboard._plain_jev_entry(cfg)
        self.assertEqual(cfg.jev_choice, 'skip')

    def test_failed_validation_declines_keep(self):
        cfg = onboard.OnboardConfig()
        with mock.patch('getpass.getpass', return_value='sk-bad'), \
             mock.patch('everett.onboard.verify_key', return_value=False), \
             mock.patch('builtins.input', return_value='n'):
            onboard._plain_jev_entry(cfg)
        self.assertEqual(cfg.jev_choice, 'skip')

    def test_failed_validation_keep_anyway(self):
        cfg = onboard.OnboardConfig()
        with mock.patch('getpass.getpass', return_value='sk-bad'), \
             mock.patch('everett.onboard.verify_key', return_value=False), \
             mock.patch('builtins.input', return_value='y'):
            onboard._plain_jev_entry(cfg)
        self.assertEqual(cfg.jev_choice, 'paste')
        self.assertFalse(cfg.jev_validated)
        self.assertEqual(cfg.jev_key, 'sk-bad')


# ---- trunk_schedule module ----------------------------------------------------------------------

class TrunkSchedulePlist(unittest.TestCase):
    def test_plist_data_shape(self):
        data = trunk_schedule.plist_data('04:00', 'claude')
        self.assertEqual(data['Label'], trunk_schedule.LABEL)
        self.assertEqual(data['StartCalendarInterval'], {'Hour': 4, 'Minute': 0})
        self.assertIn('trunk', data['ProgramArguments'])
        self.assertIn('merge', data['ProgramArguments'])
        self.assertIn('--llm', data['ProgramArguments'])
        self.assertIn('claude', data['ProgramArguments'])
        self.assertFalse(data['RunAtLoad'])

    def test_at_time_parsed(self):
        data = trunk_schedule.plist_data('23:59', 'codex')
        self.assertEqual(data['StartCalendarInterval'], {'Hour': 23, 'Minute': 59})

    def test_bad_at_raises(self):
        with self.assertRaises(ValueError):
            trunk_schedule.plist_data('not-a-time', 'claude')
        with self.assertRaises(ValueError):
            trunk_schedule.plist_data('25:00', 'claude')

    def test_render_plist_is_valid_plist_xml(self):
        import plistlib
        data = plistlib.loads(trunk_schedule.render_plist('04:00', 'none'))
        self.assertEqual(data['Label'], trunk_schedule.LABEL)


class TrunkScheduleInstallRemove(IsolatedHome):
    """Every call here goes through an injected runner -- never real launchctl -- and paths
    resolve under the sandboxed EVERETT_HOME, never the real ~/Library."""

    def test_install_writes_plist_and_calls_injected_runner(self):
        calls = []

        def fake_runner(cmd, **kwargs):
            calls.append(cmd)
            return mock.Mock(returncode=0, stderr='')

        result = trunk_schedule.install(runner=fake_runner)
        self.assertTrue(Path(result['path']).exists())
        self.assertTrue(result['path'].startswith(str(home())))
        self.assertTrue(result['launchctl_ok'])
        self.assertEqual(len(calls), 1)
        self.assertEqual(calls[0][:2], ['launchctl', 'bootstrap'])
        mode = stat.S_IMODE(Path(result['path']).stat().st_mode)
        self.assertEqual(mode, 0o644)

    def test_install_is_idempotent(self):
        fake_runner = mock.Mock(return_value=mock.Mock(returncode=0, stderr=''))
        trunk_schedule.install(runner=fake_runner)
        trunk_schedule.install(runner=fake_runner)
        self.assertEqual(fake_runner.call_count, 2)  # safe to call again; plist content is the same
        self.assertTrue(trunk_schedule.is_scheduled())

    def test_remove_uninstalls_and_calls_bootout(self):
        fake_runner = mock.Mock(return_value=mock.Mock(returncode=0, stderr=''))
        trunk_schedule.install(runner=fake_runner)
        result = trunk_schedule.remove(runner=fake_runner)
        self.assertTrue(result['removed'])
        self.assertFalse(trunk_schedule.is_scheduled())
        last_call = fake_runner.call_args_list[-1][0][0]
        self.assertEqual(last_call[:2], ['launchctl', 'bootout'])

    def test_remove_when_nothing_scheduled_is_a_noop(self):
        fake_runner = mock.Mock()
        result = trunk_schedule.remove(runner=fake_runner)
        self.assertFalse(result['removed'])
        fake_runner.assert_not_called()

    def test_never_touches_real_home(self):
        self.assertTrue(str(trunk_schedule.plist_path()).startswith(self._tmp.name))
        self.assertTrue(str(trunk_schedule.log_path()).startswith(self._tmp.name))


class TrunkScheduleCLI(IsolatedHome):
    def test_print_by_default_does_not_write(self):
        out = io.StringIO()
        with redirect_stdout(out):
            rc = main(['trunk', 'schedule'])
        self.assertEqual(rc, 0)
        self.assertFalse(trunk_schedule.plist_path().exists())
        self.assertIn('would write', out.getvalue())

    def test_apply_writes_and_uses_mocked_launchctl(self):
        with mock.patch('everett.trunk_schedule.subprocess.run') as run:
            run.return_value = mock.Mock(returncode=0, stderr='')
            out = io.StringIO()
            with redirect_stdout(out):
                rc = main(['trunk', 'schedule', '--apply', '--llm', 'codex', '--at', '05:30'])
        self.assertEqual(rc, 0)
        self.assertTrue(trunk_schedule.plist_path().exists())
        run.assert_called_once()
        self.assertIn('launchctl', run.call_args[0][0][0])
        self.assertIn('wrote', out.getvalue())

    def test_remove_apply_uninstalls(self):
        with mock.patch('everett.trunk_schedule.subprocess.run') as run:
            run.return_value = mock.Mock(returncode=0, stderr='')
            main(['trunk', 'schedule', '--apply'])
            out = io.StringIO()
            with redirect_stdout(out):
                rc = main(['trunk', 'schedule', '--remove', '--apply'])
        self.assertEqual(rc, 0)
        self.assertFalse(trunk_schedule.plist_path().exists())
        self.assertIn('removed', out.getvalue())

    def test_remove_without_apply_previews(self):
        out = io.StringIO()
        with redirect_stdout(out):
            rc = main(['trunk', 'schedule', '--remove'])
        self.assertEqual(rc, 0)
        self.assertIn('would remove', out.getvalue())

    def test_bad_at_rejected(self):
        out = io.StringIO()
        with redirect_stdout(out):
            rc = main(['trunk', 'schedule', '--at', 'bogus'])
        self.assertEqual(rc, 2)


class DoctorShowsSchedule(IsolatedHome):
    def test_doctor_reports_not_scheduled(self):
        from everett import doctor
        out = io.StringIO()
        with redirect_stdout(out):
            doctor.run()
        self.assertIn('trunk schedule: not scheduled', out.getvalue())

    def test_doctor_reports_scheduled(self):
        from everett import doctor
        with mock.patch('everett.trunk_schedule.subprocess.run') as run:
            run.return_value = mock.Mock(returncode=0, stderr='')
            trunk_schedule.install()
        out = io.StringIO()
        with redirect_stdout(out):
            doctor.run()
        self.assertIn('trunk schedule: scheduled', out.getvalue())


if __name__ == '__main__':
    unittest.main()
