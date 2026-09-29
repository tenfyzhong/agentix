"""Tests for the isolated Mem0 benchmark model adapter."""
import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).parents[1]))
spec = importlib.util.spec_from_file_location('adapter', Path(__file__).parents[1] / 'memory_mem0_codex.py')
adapter = importlib.util.module_from_spec(spec)
spec.loader.exec_module(adapter)

class Mem0CodexTests(unittest.TestCase):
    def test_messages_and_json_response_are_preserved_with_usage(self):
        with tempfile.TemporaryDirectory() as directory:
            config = SimpleNamespace(model='gpt-6-astra', reasoning_effort='low')
            llm = adapter.CodexMem0LLM(config, output_directory=directory)
            messages = [{'role':'system','content':'Extract facts.'},
                        {'role':'user','content':'I prefer hiking.'}]
            def fake(command, **kwargs):
                self.assertIn('--ignore-user-config', command)
                self.assertIn('--ephemeral', command)
                self.assertEqual(json.loads(kwargs['input']), messages[1:])
                cwd = Path(kwargs['cwd'])
                self.assertEqual((cwd/'instructions.txt').read_text(), 'Extract facts.')
                Path(command[command.index('--output-last-message')+1]).write_text('{"memory":[]}')
                kwargs['stdout'].write('{"type":"turn.completed","usage":{"input_tokens":12}}\n')
            with patch.object(adapter.subprocess, 'run', side_effect=fake):
                self.assertEqual(json.loads(llm.generate_response(messages, response_format={'type':'json_object'})), {'memory':[]})
            call = next(Path(directory).glob('call-*'))
            self.assertEqual(json.loads((call/'receipt.json').read_text())['usage']['input_tokens'],12)
            self.assertEqual(json.loads((call/'messages.json').read_text()),messages)

    @unittest.skipUnless(os.environ.get('TASKIX_BENCH_MEM0_SMOKE'), 'explicit live benchmark smoke only')
    def test_live_mem0_extraction_and_bge_m3_retrieval(self):
        from mem0 import Memory
        from mem0.utils.factory import LlmFactory
        directory = Path(os.environ['TASKIX_BENCH_MEM0_SMOKE']).resolve()
        directory.mkdir()
        self.assertEqual(os.environ.get('MEM0_TELEMETRY'), 'false')
        self.assertTrue(os.environ.get('MEM0_DIR'))
        # Mem0 validates a fixed provider-name list; override only this process's factory.
        LlmFactory.register_provider('openai', 'memory_mem0_codex.CodexMem0LLM')
        config = {
            'llm': {'provider':'openai','config':{'model':'gpt-6-astra','reasoning_effort':'low'}},
            'embedder': {'provider':'ollama','config':{'model':'bge-m3','embedding_dims':1024,
                          'ollama_base_url':'http://127.0.0.1:11435'}},
            'vector_store': {'provider':'qdrant','config':{'path':str(directory/'qdrant'),
                             'collection_name':'benchmark','embedding_model_dims':1024}},
            'history_db_path':str(directory/'history.sqlite3')}
        (directory/'config.json').write_text(json.dumps(config,indent=2))
        from mem0.utils.spacy_models import get_nlp_full, get_nlp_lemma
        self.assertIsNotNone(get_nlp_full())
        self.assertIsNotNone(get_nlp_lemma())
        memory = Memory.from_config(config)
        self.assertIsNotNone(memory.vector_store._get_bm25_encoder())
        added = memory.add([{'role':'user','content':
            'For Project Cedar, our procurement committee requires external vendor reviews on Tuesdays.'}],
            user_id='cedar')
        found = memory.search('When are external vendor reviews scheduled?',filters={'user_id':'cedar'},top_k=10)
        foreign = memory.search('When are external vendor reviews scheduled?',filters={'user_id':'other'},top_k=10)
        (directory/'results.json').write_text(json.dumps({'added':added,'found':found,'foreign':foreign},indent=2))
        keyword = memory.vector_store.keyword_search('procurement Tuesday', filters={'user_id':'cedar'}, top_k=10)
        self.assertTrue(keyword, 'BM25 must retrieve the stored decision')
        self.assertTrue(added['results'])
        self.assertTrue(any('tuesday' in item['memory'].lower() for item in found['results']))
        self.assertEqual(foreign['results'],[])

    def test_rejects_tools_and_wrong_model_before_invocation(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaises(ValueError):
                adapter.CodexMem0LLM(SimpleNamespace(model='other'),output_directory=directory)
            llm=adapter.CodexMem0LLM(SimpleNamespace(model='gpt-6-astra',reasoning_effort='low'),output_directory=directory)
            with patch.object(adapter.subprocess,'run') as run:
                with self.assertRaises(ValueError):
                    llm.generate_response([{'role':'user','content':'hello'}],tools=[{'name':'shell'}])
                run.assert_not_called()

if __name__ == '__main__':
    unittest.main()
