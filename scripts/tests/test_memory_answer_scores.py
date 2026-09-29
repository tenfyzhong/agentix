"""Tests for complete, category-aware answer evaluation."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('scores', Path(__file__).parents[1] / 'memory_answer_scores.py')
scores = importlib.util.module_from_spec(spec)
spec.loader.exec_module(scores)

class AnswerScoreTests(unittest.TestCase):
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
