"""Replay attributed source turns through pinned Mem0 OSS with atomic checkpoints."""
import argparse
import hashlib
import json
import logging
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time


def fingerprint(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, ensure_ascii=False).encode()).hexdigest()


def atomic_json(path, value):
    pending = path.with_suffix('.pending')
    with pending.open('w') as output:
        json.dump(value, output, ensure_ascii=False, indent=2)
        output.flush()
        os.fsync(output.fileno())
    pending.replace(path)


def ingest(root, sources, manifest, apply, *, resume=False):
    """Apply one source to a private copy; commit only after the store is closed."""
    identities = [(s['project'], s['id']) for s in sources]
    if not sources or len(set(identities)) != len(identities):
        raise ValueError('empty or duplicate sources')
    expected = {**manifest, 'sources_sha256': fingerprint(sources), 'sources': len(sources)}
    manifest_path = root / 'manifest.json'
    if resume:
        if json.loads(manifest_path.read_text()) != expected:
            raise ValueError('replay manifest mismatch')
    else:
        if manifest_path.exists():
            raise ValueError('existing run requires explicit resume')
        atomic_json(manifest_path, expected)
    pointer = root / 'checkpoint.json'
    state = json.loads(pointer.read_text()) if pointer.exists() else {'count': 0, 'directory': None}
    if not 0 <= state['count'] <= len(sources):
        raise ValueError('invalid checkpoint count')
    previous = root / state['directory'] if state['directory'] else None
    if previous and not (previous / 'store').is_dir():
        raise ValueError('checkpoint store missing')
    for index in range(state['count'], len(sources)):
        attempt = Path(tempfile.mkdtemp(prefix=f'source-{index:06}-', dir=root))
        store = attempt / 'store'
        if previous:
            shutil.copytree(previous / 'store', store)
        else:
            store.mkdir()
        calls = attempt / 'calls'
        calls.mkdir()
        started = time.monotonic()
        try:
            result = apply(store, sources[index], calls)
            atomic_json(attempt / 'receipt.json', {
                'source_id': sources[index]['id'], 'project': sources[index]['project'],
                'source_sha256': fingerprint(sources[index]), 'result': result,
                'latency_ms': (time.monotonic() - started) * 1000})
        except Exception as error:
            (attempt / 'error.txt').write_text(str(error))
            raise
        atomic_json(pointer, {'count': index + 1, 'directory': attempt.name})
        # Keep audit receipts/model calls, but only the latest closed store copy.
        if previous:
            shutil.rmtree(previous / 'store')
        previous = attempt
        print(f'completed extraction {index + 1}/{len(sources)}', flush=True)
    atomic_json(root / 'completion.json', {'sources': len(sources), 'sources_sha256': fingerprint(sources)})
    return previous / 'store'


def retrieval_row(question, result):
    memories = []
    seen = set()
    for item in result['results']:
        if item.get('user_id') != question['project']:
            raise ValueError('foreign project memory')
        if item['id'] in seen or not isinstance(item['memory'], str) or not item['memory'].strip():
            raise ValueError('invalid retrieved memory')
        seen.add(item['id'])
        memories.append({'id': item['id'], 'project_id': question['project'], 'content': {
            'title': '', 'conclusion': item['memory'], 'rationale': '', 'scope': '', 'conditions': []}})
    return {'id': question['id'], 'project': question['project'], 'mode': 'hybrid',
            'actual_mode': 'hybrid', 'system': 'mem0-oss', 'memories': memories}


def memory_config(store, endpoint):
    return {
        'llm': {'provider': 'openai', 'config': {'model': 'gpt-6-astra', 'reasoning_effort': 'low'}},
        'embedder': {'provider': 'ollama', 'config': {'model': 'bge-m3', 'embedding_dims': 1024,
                                                     'ollama_base_url': endpoint}},
        'vector_store': {'provider': 'qdrant', 'config': {'path': str(store / 'qdrant'),
                         'collection_name': 'benchmark', 'embedding_model_dims': 1024}},
        'history_db_path': str(store / 'history.sqlite3')}


def open_memory(store, endpoint):
    from mem0 import Memory
    memory = Memory.from_config(memory_config(store, endpoint))
    if memory.vector_store._get_bm25_encoder() is None:
        close_memory(memory)
        raise RuntimeError('Mem0 BM25 is unavailable')
    return memory


def close_memory(memory):
    try:
        memory.vector_store.client.close()
    finally:
        memory.db.close()


class WarningCapture(logging.Handler):
    def __init__(self):
        super().__init__(logging.WARNING)
        self.messages = []

    def emit(self, record):
        self.messages.append(record.getMessage())


def apply_source(store, source, calls, endpoint):
    os.environ['TASKIX_BENCH_MEM0_CALLS'] = str(calls)
    memory = open_memory(store, endpoint)
    warnings = WarningCapture()
    logger = logging.getLogger('mem0')
    logger.addHandler(warnings)
    try:
        result = memory.add([{'role': 'user', 'content': source['text']}],
                            user_id=source['project'], metadata={'source_id': source['id']})
        responses = list(calls.glob('call-*/response.json'))
        if len(responses) != 1:
            raise ValueError('expected one extraction response per source')
        extracted = json.loads(responses[0].read_text()).get('memory')
        if not isinstance(extracted, list) or any(not isinstance(item, dict)
                or not isinstance(item.get('text'), str) or not item['text'].strip() for item in extracted):
            raise ValueError('invalid Mem0 extraction response')
        if warnings.messages:
            raise RuntimeError('Mem0 reported degraded extraction: ' + '; '.join(warnings.messages))
        return result
    finally:
        logger.removeHandler(warnings)
        close_memory(memory)


def main():
    import fcntl
    import importlib.metadata
    import platform
    import urllib.request
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('corpus')
    parser.add_argument('questions')
    parser.add_argument('output')
    parser.add_argument('--project', required=True)
    parser.add_argument('--mem0-source', required=True)
    parser.add_argument('--endpoint', default='http://127.0.0.1:11435')
    parser.add_argument('--resume', action='store_true')
    args = parser.parse_args()
    root = Path(args.output).resolve()
    if not args.resume:
        root.mkdir()
    if os.environ.get('MEM0_TELEMETRY') != 'false' or not os.environ.get('MEM0_DIR'):
        raise ValueError('explicit isolated Mem0 environment required')
    with (root / 'run.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        commit = subprocess.check_output(['git', '-C', args.mem0_source, 'rev-parse', 'HEAD'], text=True).strip()
        if commit != '94c3fe9f238f3dbf29c9ce98643bd71eb13077cd':
            raise ValueError('unexpected Mem0 revision')
        from mem0.utils.factory import LlmFactory
        from mem0.utils.spacy_models import get_nlp_full, get_nlp_lemma
        import mem0.memory.main as mem0_main
        if get_nlp_full() is None or get_nlp_lemma() is None:
            raise RuntimeError('Mem0 NLP is unavailable')
        LlmFactory.register_provider('openai', 'memory_mem0_codex.CodexMem0LLM')
        with urllib.request.urlopen(args.endpoint + '/api/tags', timeout=30) as response:
            models = json.load(response)['models']
        model = next(m for m in models if m['name'] == 'bge-m3:latest')
        if model['digest'] != '7907646426070047a77226ac3e684fbbe8410524f7b4a74d02837e43f2146bab':
            raise ValueError('unexpected BGE-M3 digest')
        sources = [s for s in json.loads(Path(args.corpus).read_text()) if s['project'] == args.project]
        questions = [q for q in json.loads(Path(args.questions).read_text()) if q['project'] == args.project]
        if not questions or len({q['id'] for q in questions}) != len(questions):
            raise ValueError('empty or duplicate project questions')
        files = [Path(__file__), Path(__file__).with_name('memory_mem0_codex.py'),
                 Path(__file__).with_name('memory_reader.py'), Path(mem0_main.__file__)]
        manifest = {'project': args.project, 'mem0_commit': commit, 'python': platform.python_version(),
                    'files': {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in files},
                    'dependencies': sorted((d.metadata['Name'], d.version) for d in importlib.metadata.distributions()),
                    'questions_sha256': fingerprint(questions), 'embedding_digest': model['digest'],
                    'model': 'gpt-6-astra', 'reasoning': 'low', 'top_k': 10, 'threshold': 0.1,
                    'codex_version': subprocess.check_output(['codex', '--version'], text=True).strip(),
                    'endpoint': args.endpoint}
        # Normalize tuples before the in-memory/file equality check on resume.
        manifest = json.loads(json.dumps(manifest))
        store = ingest(root, sources, manifest,
                       lambda s, row, calls: apply_source(s, row, calls, args.endpoint), resume=args.resume)
        # Query a disposable snapshot so the committed extraction store is immutable.
        with tempfile.TemporaryDirectory(prefix='query-', dir=root) as tmp:
            snapshot = Path(tmp) / 'store'
            shutil.copytree(store, snapshot)
            memory = open_memory(snapshot, args.endpoint)
            rows = []
            try:
                for question in questions:
                    started = time.monotonic()
                    result = memory.search(question['question'], filters={'user_id': args.project},
                                           top_k=10, threshold=0.1, rerank=False)
                    row = retrieval_row(question, result)
                    row['latency_ms'] = (time.monotonic() - started) * 1000
                    rows.append(row)
            finally:
                close_memory(memory)
        pending = root / 'retrieval.pending'
        pending.write_text(''.join(json.dumps(r, ensure_ascii=False) + '\n' for r in rows))
        pending.replace(root / 'retrieval.jsonl')
        atomic_json(root / 'query-completion.json', {'questions': len(rows), 'system': 'mem0-oss'})


if __name__ == '__main__':
    main()
