"""Regression tests for the public memory benchmark protocol."""
import importlib.util
import math
import json
import sqlite3
import tempfile
from pathlib import Path
import unittest

SPEC = importlib.util.spec_from_file_location(
    'memory_benchmark', Path(__file__).parents[1] / 'memory_benchmark.py')
benchmark = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(benchmark)


class BenchmarkTests(unittest.TestCase):
    def test_corpus_excludes_gold_and_sorts_sessions_numerically(self):
        sample = {'sample_id': 'conv-26', 'conversation': {
            'session_10': [{'dia_id': 'D10:1', 'speaker': 'B', 'text': 'Later'}],
            'session_2': [{'dia_id': 'D2:1', 'speaker': 'A', 'text': 'Earlier',
                           'blip_caption': 'A boat', 'query': 'SECRET_QUERY'}],
            'session_2_date_time': '1 May 2023',
        }, 'qa': [{'answer': 'SECRET_GOLD'}], 'observation': 'SECRET_SUMMARY'}
        rows = benchmark.dialogues(sample)
        self.assertEqual([r['id'] for r in rows], ['D2:1', 'D10:1'])
        self.assertIn('A boat', rows[0]['text'])
        self.assertIn('1 May 2023', rows[0]['text'])
        self.assertNotIn('SECRET', str(rows))

    def test_source_receipts_preserve_turns_without_gold_or_future_context(self):
        sample = {'sample_id': 'conv-26', 'conversation': {
            'session_1_date_time': '1 May 2023', 'session_1': [
                {'dia_id': 'D1:1', 'speaker': 'A', 'text': 'First claim'},
                {'dia_id': 'D1:2', 'speaker': 'B', 'text': 'Second claim'}]},
            'qa': [{'answer': 'SECRET_GOLD'}]}
        sources = benchmark.source_receipts(sample)
        self.assertEqual(len(sources), 2)
        self.assertEqual([s['sequence'] for s in sources], [1, 2])
        self.assertEqual(sources[0]['messages'][0]['id'], 'D1:1')
        self.assertEqual(sources[1]['messages'][0]['role'], 'user')
        self.assertIn('B: Second claim', sources[1]['messages'][0]['text'])
        self.assertIn('1 May 2023', sources[0]['messages'][0]['text'])
        self.assertNotIn('Second claim', str(sources[0]))
        self.assertNotIn('SECRET_GOLD', str(sources))
        self.assertEqual(sources[0]['session_id'], sources[1]['session_id'])
        self.assertNotEqual(sources[0]['receipt_id'], sources[1]['receipt_id'])

    def test_policy_export_keeps_gold_out_and_preserves_message_roles(self):
        cases = [{'id': 'scope', 'required': ['SECRET_GOLD'], 'forbidden': ['SECRET_BAD'],
                  'turns': [[{'role': 'assistant', 'text': 'Consider remote hosting'}],
                            [{'role': 'user', 'text': 'Choose offline for Acorn only'}]]}]
        sources = benchmark.policy_receipts(cases)
        self.assertEqual(len(sources), 2)
        self.assertEqual(sources[0]['messages'][0]['role'], 'assistant')
        self.assertNotIn('SECRET', str(sources))
        self.assertEqual(sources[0]['project_id'], sources[1]['project_id'])
        self.assertEqual(sources[1]['messages'][0]['id'], 'turn-2-message-1')

    def test_extraction_audit_rejects_incomplete_sources_and_invalid_quotes(self):
        sources = benchmark.policy_receipts([{'id': 'audit', 'turns': [
            [{'role': 'user', 'text': 'Choose offline for Acorn only'}]]}])
        source = sources[0]
        with tempfile.TemporaryDirectory() as directory:
            database = Path(directory) / 'memory.db'
            with sqlite3.connect(database) as connection:
                connection.executescript("""
                    CREATE TABLE sources(receipt_id TEXT, data TEXT);
                    CREATE TABLE work_items(receipt_id TEXT, kind TEXT, state TEXT,
                        payload TEXT, attempts INTEGER, error TEXT);
                    CREATE TABLE memories(data TEXT);
                """)
                connection.execute('INSERT INTO sources VALUES (?, ?)',
                                   (source['receipt_id'], json.dumps(source)))
                connection.execute('INSERT INTO work_items VALUES (?, ?, ?, ?, ?, ?)',
                    (source['receipt_id'], 'extract', 'pending',
                     json.dumps({'message_id': 'turn-1-message-1'}), 0, None))
            report, memories = benchmark.audit_extraction(sources, database)
            self.assertFalse(report['complete'])
            self.assertEqual(report['work_states']['pending'], 1)
            with sqlite3.connect(database) as connection:
                connection.execute("UPDATE work_items SET state='done'")
            self.assertTrue(benchmark.audit_extraction(sources, database)[0]['complete'])
            missing = dict(source, receipt_id='missing')
            report, _ = benchmark.audit_extraction(sources + [missing], database)
            self.assertEqual(report['missing_receipts'], ['missing'])
            memory = {'id': 'm', 'project_id': source['project_id'], 'status': 'active',
                      'content': {'valid_until': None, 'evidence': [{
                          'receipt_id': source['receipt_id'],
                          'message_id': 'turn-1-message-1', 'quote': 'fabricated'}]}}
            with sqlite3.connect(database) as connection:
                connection.execute('INSERT INTO memories VALUES (?)', (json.dumps(memory),))
            report, _ = benchmark.audit_extraction(sources, database)
            self.assertFalse(report['complete'])
            self.assertEqual(report['invalid_evidence'][0]['memory_id'], 'm')

    def test_duplicate_evidence_does_not_inflate_recall(self):
        score = benchmark.retrieval_metrics([['a'], ['a'], ['b']], ['a', 'b'], 2)
        self.assertEqual(score['recall'], .5)
        self.assertEqual(score['all_evidence'], 0)
        self.assertEqual(score['mrr'], 1)
        self.assertAlmostEqual(score['ndcg'], 1 / (1 + 1 / math.log2(3)))

    def test_empty_gold_is_unscored_not_perfect(self):
        self.assertIsNone(benchmark.retrieval_metrics([['a']], [], 5))

    def test_report_rejects_missing_and_duplicate_questions(self):
        questions = [{'id': 'q', 'split': 'development', 'category': 1,
                      'evidence': ['a'], 'invalid_evidence': []}]
        with self.assertRaises(ValueError):
            benchmark.score_rows(questions, [], ['fts'])
        result = {'id': 'q', 'mode': 'fts', 'ids': ['a']}
        with self.assertRaises(ValueError):
            benchmark.score_rows(questions, [result, result], ['fts'])
        report = benchmark.score_rows(questions, [result], ['fts'])
        self.assertEqual(report['fts/development/10']['recall'], 1)
        self.assertEqual(report['fts/development/10']['n'], 1)

    def test_empty_ranking_has_zero_scores(self):
        self.assertEqual(set(benchmark.retrieval_metrics([], ['a'], 5).values()), {0})

    def test_duplicate_dialogue_ids_are_rejected(self):
        sample = {'sample_id': 'x', 'conversation': {'session_1': [
            {'dia_id': 'a', 'speaker': 'A', 'text': 'one'},
            {'dia_id': 'a', 'speaker': 'B', 'text': 'two'}]}}
        with self.assertRaises(ValueError):
            benchmark.dialogues(sample)


if __name__ == '__main__':
    unittest.main()
