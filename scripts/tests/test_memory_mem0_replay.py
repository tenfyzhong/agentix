"""Recovery and isolation contracts for the matched Mem0 replay."""
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).parents[1]))
spec = importlib.util.spec_from_file_location('replay', Path(__file__).parents[1] / 'memory_mem0_replay.py')
replay = importlib.util.module_from_spec(spec)
spec.loader.exec_module(replay)


class ReplayTests(unittest.TestCase):
    def test_failed_write_is_not_committed_and_resume_replays_only_missing_source(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sources = [{'id': 'a', 'project': 'p', 'text': 'first'},
                       {'id': 'b', 'project': 'p', 'text': 'second'}]
            seen = []
            fail = [True]

            def apply(store, source, calls):
                state = store / 'values.json'
                values = json.loads(state.read_text()) if state.exists() else []
                values.append(source['id'])
                state.write_text(json.dumps(values))
                seen.append(source['id'])
                if source['id'] == 'b' and fail[0]:
                    raise RuntimeError('interrupted after write')
                return {'results': []}

            with self.assertRaisesRegex(RuntimeError, 'interrupted'):
                replay.ingest(root, sources, {'version': 1}, apply)
            self.assertFalse((root / 'completion.json').exists())
            fail[0] = False
            store = replay.ingest(root, sources, {'version': 1}, apply, resume=True)
            self.assertEqual(json.loads((store / 'values.json').read_text()), ['a', 'b'])
            self.assertEqual(seen, ['a', 'b', 'b'])
            self.assertEqual(json.loads((root / 'completion.json').read_text())['sources'], 2)
            with self.assertRaisesRegex(ValueError, 'manifest'):
                replay.ingest(root, sources, {'version': 2}, apply, resume=True)

    def test_source_order_and_duplicate_identity_are_guarded(self):
        with tempfile.TemporaryDirectory() as tmp:
            sources = [{'id': 'a', 'project': 'p', 'text': 'first'}]
            root = Path(tmp)
            replay.ingest(root, sources, {}, lambda *args: {'results': []})
            with self.assertRaisesRegex(ValueError, 'manifest'):
                replay.ingest(root, [{**sources[0], 'text': 'changed'}], {}, lambda *args: None, resume=True)
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaisesRegex(ValueError, 'duplicate'):
                replay.ingest(Path(tmp), sources * 2, {}, lambda *args: None)

    def test_export_rejects_foreign_project_and_preserves_only_memory_text(self):
        question = {'id': 'q', 'project': 'p'}
        result = {'results': [{'id': 'm', 'memory': 'Review on Tuesday', 'user_id': 'p',
                               'metadata': {'raw_quote': 'must not leak'}}]}
        row = replay.retrieval_row(question, result)
        self.assertEqual(row['actual_mode'], 'hybrid')
        self.assertEqual(row['memories'][0]['content']['conclusion'], 'Review on Tuesday')
        self.assertNotIn('must not leak', json.dumps(row))
        result['results'][0]['user_id'] = 'other'
        with self.assertRaisesRegex(ValueError, 'foreign'):
            replay.retrieval_row(question, result)


if __name__ == '__main__':
    unittest.main()
