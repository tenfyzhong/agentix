"""Reproducible LoCoMo adapters and evidence ranking metrics.

Gold annotations are deliberately separate from corpus export and embeddings.
"""
import argparse
import hashlib
import json
import math
from pathlib import Path
import urllib.request


def dialogues(sample):
    conversation = sample['conversation']
    sessions = sorted((k for k, v in conversation.items()
                       if k.startswith('session_') and isinstance(v, list)),
                      key=lambda k: int(k.split('_')[1]))
    rows, seen = [], set()
    for session in sessions:
        date = conversation.get(session + '_date_time', '')
        for turn in conversation[session]:
            identity = turn['dia_id']
            if identity in seen:
                raise ValueError(f'duplicate dialogue ID: {identity}')
            seen.add(identity)
            text = f"{date} | {turn['speaker']}: {turn['text']}"
            if turn.get('blip_caption'):
                text += '\nImage caption: ' + turn['blip_caption']
            rows.append({'id': identity, 'text': text, 'session': session})
    return rows


def retrieval_metrics(ranked_evidence, gold, k):
    gold = set(gold)
    if not gold:
        return None
    seen, covered, dcg, first = set(), set(), 0.0, 0
    for rank, evidence in enumerate(ranked_evidence[:k], 1):
        new = (set(evidence) & gold) - seen
        if new:
            if not first:
                first = rank
            # Binary relevance per ranked record; repeat citations get no credit.
            dcg += 1 / math.log2(rank + 1)
        covered.update(new)
        seen.update(evidence)
    ideal = sum(1 / math.log2(i + 2) for i in range(min(k, len(gold))))
    return {'recall': len(covered) / len(gold), 'hit': int(bool(covered)),
            'all_evidence': int(covered == gold), 'mrr': 1 / first if first else 0,
            'ndcg': dcg / ideal if ideal else 0}


def prepare(dataset, output):
    """Export source-only corpus and a separate scoring file."""
    raw = Path(dataset).read_bytes()
    data = json.loads(raw)
    corpus, questions = [], []
    for sample in data:
        project = sample['sample_id']
        rows = dialogues(sample)
        corpus.extend(dict(row, project=project) for row in rows)
        valid = {r['id'] for r in rows}
        for index, qa in enumerate(sample['qa']):
            evidence = qa.get('evidence', [])
            questions.append({'id': f'{project}/{index}', 'project': project,
                              'split': 'development' if project in ('conv-26', 'conv-30') else 'held_out',
                              'question': qa['question'], 'category': qa['category'],
                              'answer': qa.get('answer'), 'evidence': evidence,
                              'invalid_evidence': sorted(set(evidence) - valid)})
    output = Path(output)
    output.mkdir(parents=True, exist_ok=True)
    (output / 'corpus.json').write_text(json.dumps(corpus, ensure_ascii=False))
    (output / 'questions.json').write_text(json.dumps(questions, ensure_ascii=False))
    manifest = {'dataset_sha256': hashlib.sha256(raw).hexdigest(),
                'conversations': len(data), 'documents': len(corpus), 'questions': len(questions),
                'questions_with_invalid_evidence': sum(bool(q['invalid_evidence']) for q in questions)}
    (output / 'manifest.json').write_text(json.dumps(manifest, indent=2))
    print(json.dumps(manifest))


def embed(input_path, output, endpoint, model):
    """Cache real vectors by text hash, resuming only identical requests."""
    rows = json.loads(Path(input_path).read_text())
    cache = {}
    output = Path(output)
    if output.exists():
        for line in output.read_text().splitlines():
            item = json.loads(line)
            if item['model'] != model or item['endpoint'] != endpoint:
                raise ValueError('embedding cache model or endpoint mismatch')
            cache[item['sha256']] = item
    with output.open('a') as stream:
        for start in range(0, len(rows), 32):
            batch = rows[start:start + 32]
            texts = [row.get('text', row.get('question')) for row in batch]
            pending = [(text, hashlib.sha256(text.encode()).hexdigest()) for text in texts]
            pending = list(dict.fromkeys((text, key) for text, key in pending if key not in cache))
            if not pending:
                continue
            body = json.dumps({'model': model, 'input': [text for text, _ in pending],
                               'truncate': False}).encode()
            request = urllib.request.Request(endpoint.rstrip('/') + '/api/embed', data=body,
                                             headers={'Content-Type': 'application/json'})
            with urllib.request.urlopen(request, timeout=180) as response:
                vectors = json.load(response)['embeddings']
            if len(vectors) != len(pending):
                raise ValueError('embedding cardinality mismatch')
            for (_, key), vector in zip(pending, vectors):
                if len(vector) != 1024 or not all(math.isfinite(v) for v in vector):
                    raise ValueError('invalid BGE-M3 vector')
                item = {'sha256': key, 'model': model, 'endpoint': endpoint, 'vector': vector}
                stream.write(json.dumps(item) + '\n')
                cache[key] = item
            stream.flush()
            print(f'embedded {min(start + 32, len(rows))}/{len(rows)}', flush=True)


def score_rows(questions, results, modes):
    expected = {(q['id'], mode) for q in questions for mode in modes}
    actual = {(r['id'], r['mode']) for r in results}
    if len(actual) != len(results) or actual != expected:
        raise ValueError('incomplete, duplicate, or unexpected question results')
    gold = {q['id']: q for q in questions}
    groups = {}
    for result in results:
        question = gold[result['id']]
        if question['category'] == 5 or question['invalid_evidence'] or not question['evidence']:
            continue
        for k in (5, 10, 20):
            score = retrieval_metrics([[identity] for identity in result['ids']], question['evidence'], k)
            for split in (question['split'], 'all'):
                key = f"{result['mode']}/{split}/{k}"
                groups.setdefault(key, []).append(score)
    return {key: dict(n=len(rows), **{metric: sum(r[metric] for r in rows) / len(rows)
                                     for metric in rows[0]})
            for key, rows in sorted(groups.items())}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    prepare_parser = commands.add_parser('prepare')
    prepare_parser.add_argument('dataset')
    prepare_parser.add_argument('output')
    embed_parser = commands.add_parser('embed')
    embed_parser.add_argument('input')
    embed_parser.add_argument('output')
    embed_parser.add_argument('--endpoint', default='http://127.0.0.1:11435')
    embed_parser.add_argument('--model', default='bge-m3')
    score_parser = commands.add_parser('score')
    score_parser.add_argument('questions')
    score_parser.add_argument('results')
    score_parser.add_argument('output')
    score_parser.add_argument('--modes', default='fts,hybrid')
    args = parser.parse_args()
    if args.command == 'prepare':
        prepare(args.dataset, args.output)
    elif args.command == 'score':
        questions = json.loads(Path(args.questions).read_text())
        results = [json.loads(line) for line in Path(args.results).read_text().splitlines()]
        report = score_rows(questions, results, args.modes.split(','))
        Path(args.output).write_text(json.dumps(report, indent=2))
        print(json.dumps(report, indent=2))
    else:
        embed(args.input, args.output, args.endpoint, args.model)


if __name__ == '__main__':
    main()
