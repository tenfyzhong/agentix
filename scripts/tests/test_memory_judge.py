"""Semantic grading must be blind to retrieval mode and bound to reader inputs."""
import importlib.util
import sys
from pathlib import Path
import unittest

sys.path.insert(0, str(Path(__file__).parents[1]))
spec = importlib.util.spec_from_file_location('judge', Path(__file__).parents[1] / 'memory_judge.py')
judge = importlib.util.module_from_spec(spec)
spec.loader.exec_module(judge)


class JudgeTests(unittest.TestCase):
    def test_judge_separates_reference_from_retrieved_evidence(self):
        question = {'id': 'q', 'question': 'Where?', 'answer': 'Ireland', 'category': 4}
        reader = {'question': 'Where?', 'context': [{'id': 'm', 'text': 'Germany'}]}
        reply = {'id': 'q', 'mode': 'hybrid', 'answer': {'answer': 'Ireland', 'abstained': False}}
        payload = judge.judge_payload(question, reply, reader)
        self.assertEqual(payload['reference_answer'], 'Ireland')
        self.assertEqual(payload['retrieved_context'], reader['context'])
        self.assertNotIn('mode', payload)
        self.assertNotIn('id', payload)
        with self.assertRaises(ValueError):
            judge.judge_payload(question, reply, {'question': 'Who?', 'context': []})

    def test_grade_requires_explained_boolean_labels(self):
        judge.validate_grade({'correct': True, 'faithful': False, 'explanation': 'Unsupported by context'})
        for value in [{'correct': 'yes', 'faithful': False, 'explanation': 'x'},
                      {'correct': True, 'faithful': False, 'explanation': ''}]:
            with self.assertRaises(ValueError):
                judge.validate_grade(value)


if __name__ == '__main__':
    unittest.main()
