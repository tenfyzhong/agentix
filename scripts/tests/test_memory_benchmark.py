"""Regression tests for the public memory benchmark protocol."""
import importlib.util
import math
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
