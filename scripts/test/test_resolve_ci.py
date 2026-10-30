from datetime import datetime, timezone
import unittest
from unittest.mock import patch

from test_package import module


class ResolveTests(unittest.TestCase):
    def run_fixture(self, **changes):
        run = {'id': 12, 'event': 'push', 'head_sha': 'sha', 'head_repository': {'full_name': 'owner/repo'},
               'status': 'completed', 'conclusion': 'success', 'html_url': 'https://example/run',
               'updated_at': '2026-10-29T00:00:00Z'}
        return run | changes

    def test_exact_push_source_is_required(self):
        ci = module('resolve_ci')
        run = self.run_fixture()
        pages = [{'workflow_runs': [run, self.run_fixture(id=13, event='pull_request'),
                                   self.run_fixture(id=14, head_sha='other'),
                                   self.run_fixture(id=15, head_repository={'full_name': 'fork/repo'})]}]
        self.assertEqual(ci.select_run(pages, 'owner/repo', 'sha')['id'], 12)

    def test_expiry_missing_and_duplicate_platforms(self):
        ci = module('resolve_ci')
        items = [{'name': 'installer-' + p, 'expired': False} for p in ci.PLATFORMS]
        now = datetime(2026, 10, 30, tzinfo=timezone.utc)
        self.assertEqual(ci.artifact_state([{'artifacts': items}], now, '2026-10-29T00:00:00Z'), 'reuse')
        self.assertEqual(ci.artifact_state([{'artifacts': []}], now, '2026-10-01T00:00:00Z'), 'rebuild')
        with self.assertRaisesRegex(ValueError, 'missing platform'):
            ci.artifact_state([{'artifacts': items[:1]}], now, '2026-10-29T00:00:00Z')
        with self.assertRaisesRegex(ValueError, 'Duplicate'):
            ci.artifact_state([{'artifacts': items + items[:1]}], now, '2026-10-29T00:00:00Z')
        items[0]['expired'] = True
        self.assertEqual(ci.artifact_state([{'artifacts': items}], now, '2026-10-29T00:00:00Z'), 'rebuild')

    def test_missing_failed_and_timed_out_runs_do_not_publish(self):
        ci = module('resolve_ci')
        with patch.object(ci, 'api', return_value=[{'workflow_runs': []}]):
            with self.assertRaisesRegex(ValueError, 'push the commit'):
                ci.resolve('owner/repo', 'sha', missing_timeout=0)
        with patch.object(ci, 'api', return_value=[{'workflow_runs': [self.run_fixture(conclusion='failure')]}]):
            with self.assertRaisesRegex(ValueError, 'did not succeed'):
                ci.resolve('owner/repo', 'sha')
        with patch.object(ci, 'api', return_value=[{'workflow_runs': [self.run_fixture(status='in_progress')]}]):
            with self.assertRaisesRegex(TimeoutError, 'Timed out'):
                ci.resolve('owner/repo', 'sha', timeout=0)
