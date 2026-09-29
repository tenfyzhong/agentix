"""Reader benchmark isolation and artifact validation tests."""
import importlib.util
import json
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('reader', Path(__file__).parents[1] / 'memory_reader.py')
reader = importlib.util.module_from_spec(spec)
spec.loader.exec_module(reader)

class ReaderTests(unittest.TestCase):
    def test_payload_excludes_gold_and_applies_same_budget(self):
        question = {'id': 'q', 'project': 'p', 'question': 'Why?', 'answer': 'SECRET_GOLD', 'evidence': ['GOLD_ID']}
        result = {'ids': ['a', 'b']}
        corpus = {('p', 'a'): 'first source', ('p', 'b'): 'second source'}
        payload = reader.reader_payload(question, result, corpus, 1, 100)
        self.assertEqual(payload['context'], [{'id': 'a', 'text': 'first source'}])
        self.assertNotIn('SECRET', json.dumps(payload))
        self.assertNotIn('GOLD_ID', json.dumps(payload))
        payload = reader.reader_payload(question, result, corpus, 2, 5)
        self.assertEqual(payload['context'], [])

    def test_extracted_context_uses_claims_not_evidence_quotes(self):
        question = {'question': 'Why?', 'project': 'p'}
        result = {'memories': [{'id': 'm', 'project_id': 'p', 'content': {
            'title': 'Decision', 'conclusion': 'Offline', 'rationale': 'Contract',
            'scope': 'production', 'conditions': [], 'evidence': [{'quote': 'HIDDEN_DETAIL'}]}}]}
        payload = reader.reader_payload(question, result, {}, 10, 1000)
        self.assertNotIn('HIDDEN_DETAIL', json.dumps(payload))
        self.assertIn('Contract', json.dumps(payload))

    def test_native_tool_and_invented_citation_are_rejected(self):
        event = {'type': 'item.completed', 'item': {'type': 'command_execution'}}
        with self.assertRaises(ValueError):
            reader.validate_events(json.dumps(event))
        with self.assertRaises(ValueError):
            reader.validate_answer({'answer':'claim','abstained':False,'citations':['foreign']}, {'context':[{'id':'a'}]})
        reader.validate_answer({'answer':'Unknown','abstained':True,'citations':[]}, {'context':[]})

if __name__ == '__main__':
    unittest.main()
