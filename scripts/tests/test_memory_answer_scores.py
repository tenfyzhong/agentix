"""Tests for complete, category-aware answer evaluation."""
import importlib.util
import os
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('scores', Path(__file__).parents[1] / 'memory_answer_scores.py')
scores = importlib.util.module_from_spec(spec)
spec.loader.exec_module(scores)

class AnswerScoreTests(unittest.TestCase):
    @unittest.skipUnless(importlib.util.find_spec('nltk'), 'optional LoCoMo scoring dependency')
    def test_locomo_stemming_and_category_specific_reference_rules(self):
        self.assertEqual(scores.locomo_score('camped and hikes', 'camping hike', 4), 1)
        self.assertEqual(scores.locomo_score('red', 'red, blue', 1), 0.5)
        self.assertEqual(scores.locomo_score('red, blue, green', 'red, blue', 1), 1)
        self.assertEqual(scores.locomo_score('doctor', 'doctor; nurse', 3), 1)
        self.assertEqual(scores.locomo_score('', '', 4), 0)

    def test_locomo_adversarial_phrase_rule_is_separate_from_abstention(self):
        self.assertEqual(scores.locomo_score('Not mentioned in the conversation', None, 5), 1)
        self.assertEqual(scores.locomo_score('Unknown', None, 5), 0)
        with self.assertRaises(ValueError):
            scores.locomo_score('red', 'red', 0)

    @unittest.skipUnless(importlib.util.find_spec('nltk'), 'optional LoCoMo scoring dependency')
    def test_locomo_metric_is_opt_in_and_does_not_rewrite_reader_output(self):
        questions = [{'id':'q','project':'p','category':4,'answer':'camping'}]
        answers = [{'id':'q','mode':'fts','answer':{'answer':'camped','abstained':True}}]
        ordinary = scores.score_answers(questions, answers, ['fts'])
        self.assertNotIn('locomo_f1', ordinary['fts']['non_adversarial'])
        report = scores.score_answers(questions, answers, ['fts'], official_f1=True)
        self.assertEqual(report['fts']['non_adversarial']['locomo_f1'], 1)
        self.assertEqual(report['fts']['non_adversarial']['token_f1'], 0)

    @unittest.skipUnless(os.environ.get('LOCOMO_EVALUATOR'), 'optional pinned upstream verification')
    def test_compatibility_against_pinned_upstream_evaluator(self):
        import ast
        from collections import Counter
        import contextlib
        import hashlib
        import io
        import string
        import numpy as np
        import regex
        from nltk.stem import PorterStemmer
        source = Path(os.environ['LOCOMO_EVALUATOR']).read_bytes()
        self.assertEqual(hashlib.sha256(source).hexdigest(),
                         '8e3be5d57ff2ff9ec5cd05939592f468c5f3f1fd95d13e431932bdf6bf0fd6fd')
        names = {'normalize_answer', 'f1_score', 'f1', 'eval_question_answering'}
        tree = ast.parse(source)
        tree.body = [node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name in names]
        self.assertEqual({node.name for node in tree.body}, names)
        namespace = {'Counter': Counter, 'np': np, 'regex': regex,
                     'string': string, 'ps': PorterStemmer()}
        exec(compile(tree, '<pinned-locomo-evaluator>', 'exec'), namespace)
        samples = ['', 'camped and hikes', 'camping hike', 'red, blue, green',
                   'doctor; nurse', 'Not mentioned', 'no information available',
                   'The café and 中文', '123_foo-bar']
        rows = [{'category': c, 'answer': gold, 'prediction': prediction}
                for c in range(1, 6) for gold in samples for prediction in samples]
        with contextlib.redirect_stdout(io.StringIO()):
            expected, _, _ = namespace['eval_question_answering'](rows)
        for row, reference_score in zip(rows, expected):
            with self.subTest(row=row):
                self.assertAlmostEqual(scores.locomo_score(row['prediction'], row['answer'], row['category']),
                                       reference_score, places=14)

    def test_f1_handles_overlap_and_empty_answers(self):
        self.assertEqual(scores.token_f1('The red car.', 'red car'), 1)
        self.assertAlmostEqual(scores.token_f1('red', 'red car'), 2/3)
        self.assertEqual(scores.token_f1('', ''), 0)

    def test_complete_denominators_separate_adversarial_abstention(self):
        questions = [{'id':'q1','project':'p','category':1,'answer':'red car'},
                     {'id':'q2','project':'p','category':5,'answer':None}]
        answers = [{'id':'q1','mode':'fts','answer':{'answer':'red car','abstained':False}},
                   {'id':'q2','mode':'fts','answer':{'answer':'Unknown','abstained':True}}]
        report = scores.score_answers(questions, answers, ['fts'])
        self.assertEqual(report['fts']['non_adversarial']['n'], 1)
        self.assertEqual(report['fts']['non_adversarial']['token_f1'], 1)
        self.assertEqual(report['fts']['adversarial']['n'], 1)
        self.assertEqual(report['fts']['adversarial']['abstention_rate'], 1)
        with self.assertRaises(ValueError):
            scores.score_answers(questions, answers[:1], ['fts'])
        with self.assertRaises(ValueError):
            scores.score_answers(questions, answers + answers, ['fts'])

    def test_partial_judge_labels_cannot_become_a_full_score(self):
        questions = [{'id':'q','project':'p','category':4,'answer':'blue'}]
        answers = [{'id':'q','mode':'fts','answer':{'answer':'blue','abstained':False}}]
        with self.assertRaises(ValueError):
            scores.score_answers(questions, answers, ['fts'], [])
        grades = [{'id':'q','mode':'fts','correct':True,'faithful':False}]
        report = scores.score_answers(questions, answers, ['fts'], grades)
        self.assertEqual(report['fts']['non_adversarial']['semantic_accuracy'], 1)
        self.assertEqual(report['fts']['non_adversarial']['faithfulness'], 0)

if __name__ == '__main__':
    unittest.main()
