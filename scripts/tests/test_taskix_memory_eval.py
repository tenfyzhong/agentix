"""Deterministic regression coverage for memory screening calibration."""
import importlib.util
from pathlib import Path
import unittest

PATH = Path(__file__).resolve().parents[1] / 'taskix-memory-eval.py'


class CalibrationTests(unittest.TestCase):
    def setUp(self):
        spec = importlib.util.spec_from_file_location('memory_eval', PATH)
        self.module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.module)

    def test_threshold_grid_uses_requested_step(self):
        self.assertEqual(self.module.THRESHOLDS,
                         [.95, .90, .85, .80, .75, .70, .65, .60, .55, .50])

    def test_negative_gate_requires_confident_skip(self):
        answer = {'type': 'choice', 'choice': 'skip', 'confidence': .9,
                  'probabilities': {'skip': .8, 'extract': .1, 'uncertain': .1}}
        self.assertTrue(self.module.skip(answer, .8))
        self.assertFalse(self.module.skip(answer, .85))
        answer['choice'] = 'extract'
        self.assertFalse(self.module.skip(answer, .8))
        answer['choice'] = 'uncertain'
        self.assertFalse(self.module.skip(answer, .8))
        self.assertFalse(self.module.skip({}, .5))

    def test_malformed_answers_fail_open(self):
        for answer in [None, [], "skip", {"probabilities": []}]:
            self.assertFalse(self.module.skip(answer, .5))

    def test_uses_production_question(self):
        question = self.module.questions()["memory_triage"]
        self.assertEqual(set(question["criteria"]), {"skip", "extract", "uncertain"})
        self.assertIn("untrusted data", question["instructions"])

    def test_recommend_max_safe_coverage_then_highest_threshold(self):
        rows = [{'threshold': .95, 'false_skip': 0, 'true_skip': 1},
                {'threshold': .9, 'false_skip': 0, 'true_skip': 3},
                {'threshold': .85, 'false_skip': 0, 'true_skip': 3},
                {'threshold': .8, 'false_skip': 1, 'true_skip': 4}]
        self.assertEqual(self.module.recommend(rows), .9)
        self.assertIsNone(self.module.recommend([{'threshold': .5, 'false_skip': 1, 'true_skip': 9}]))

    def test_chunk_offsets_are_utf8_bytes(self):
        chunks = list(self.module.chunks('中' * 6000))
        self.assertEqual(chunks[0][1], 0)
        self.assertEqual(chunks[1][1], 16383)
        self.assertEqual(''.join(c[0] for c in chunks), '中' * 6000)
