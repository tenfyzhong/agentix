"""Reproducible LoCoMo adapters and evidence ranking metrics.

Gold annotations are deliberately separate from corpus export and embeddings.
"""
import argparse
import hashlib
import json
import math
import sqlite3
from collections import Counter
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


def source_receipts(sample):
    """One attributed human-source receipt per dialogue turn, in original order.

    Receipt timestamps are deterministic ordering values, not event dates. Event
    dates remain in the message itself. Both persona speakers are human sources.
    """
    project = sample['sample_id']
    return [{'instance_id': 'locomo-v1', 'receipt_id': f"{project}/{row['id']}",
             'sequence': sequence, 'project_id': project, 'session_id': project,
             'turn_id': row['id'], 'revision': 1, 'job_id': None,
             'recorded_at': 1682899200 + sequence,
             'messages': [{'id': row['id'], 'role': 'user', 'text': row['text']}]}
            for sequence, row in enumerate(dialogues(sample), 1)]


def policy_receipts(cases):
    """Export only the authored dialogue, never expected or forbidden memories."""
    sources = []
    for case in cases:
        project = 'policy-' + case['id']
        for turn, messages in enumerate(case['turns'], 1):
            sources.append({'instance_id': 'project-policy-v1',
                            'receipt_id': f'{project}/{turn}', 'sequence': turn,
                            'project_id': project, 'session_id': project,
                            'turn_id': str(turn), 'revision': 1, 'job_id': None,
                            'recorded_at': 1735689600 + turn,
                            'messages': [{'id': f'turn-{turn}-message-{index}',
                                          'role': message['role'], 'text': message['text']}
                                         for index, message in enumerate(messages, 1)]})
    return sources


def audit_extraction(sources, database):
    """Read one SQLite snapshot; audit coverage separately from semantic quality."""
    expected = {source['receipt_id']: source for source in sources}
    if not expected or len(expected) != len(sources):
        raise ValueError('empty or duplicate source receipts')
    with sqlite3.connect(Path(database).resolve().as_uri() + '?mode=ro', uri=True) as connection:
        connection.execute('BEGIN')
        now = connection.execute('SELECT unixepoch()').fetchone()[0]
        actual = {identity: json.loads(data) for identity, data in
                  connection.execute('SELECT receipt_id, data FROM sources')}
        work = list(connection.execute(
            'SELECT receipt_id, kind, state, payload, attempts, error FROM work_items'))
        memories = [json.loads(row[0]) for row in connection.execute('SELECT data FROM memories')]
    missing = sorted(set(expected) - set(actual))
    unexpected = sorted(set(actual) - set(expected))
    changed = sorted(identity for identity in expected.keys() & actual.keys()
                     if expected[identity] != actual[identity])
    states = Counter(row[2] for row in work)
    processed = {(row[0], json.loads(row[3]).get('message_id'))
                 for row in work if row[1] == 'extract' and row[2] == 'done'}
    missing_messages = [{'receipt_id': identity, 'message_id': message['id']}
                        for identity, source in expected.items() for message in source['messages']
                        if message['text'] and (identity, message['id']) not in processed]
    invalid, cited, eligible = [], set(), []
    for memory in memories:
        content = memory['content']
        if memory['status'] not in ('active', 'conflicted'):
            continue
        if content.get('valid_until') is not None and content['valid_until'] <= now:
            continue
        eligible.append(memory)
        if not content.get('evidence'):
            invalid.append({'memory_id': memory['id'], 'reason': 'missing evidence'})
        for evidence in content.get('evidence', []):
            source = actual.get(evidence['receipt_id'])
            message = next((m for m in source['messages'] if m['id'] == evidence['message_id']), None) if source else None
            if not source or source['project_id'] != memory['project_id'] or not message or not evidence['quote'] or evidence['quote'] not in message['text']:
                invalid.append({'memory_id': memory['id'], 'evidence': evidence})
            else:
                cited.add((memory['project_id'], evidence['receipt_id'], evidence['message_id']))
    failures = [{'receipt_id': row[0], 'kind': row[1], 'attempts': row[4], 'error': row[5]}
                for row in work if row[2] == 'failed']
    complete = not (missing or unexpected or changed or missing_messages or invalid or
                    any(state != 'done' and count for state, count in states.items()))
    report = {'complete': complete, 'snapshot_at': now, 'expected_receipts': len(expected),
              'received_receipts': len(actual), 'missing_receipts': missing,
              'unexpected_receipts': unexpected, 'changed_receipts': changed,
              'missing_completed_messages': missing_messages, 'work_states': dict(states),
              'failed_work': failures, 'retried_work': sum(row[4] > 1 for row in work),
              'memory_states': dict(Counter(m['status'] for m in memories)),
              'retrievable_memories': len(eligible), 'invalid_evidence': invalid,
              'cited_source_messages': len(cited)}
    return report, eligible


def score_policy_grades(cases, grades):
    """Aggregate explicit semantic labels; never infer them from substring matches."""
    expected = {case['id']: case for case in cases}
    observed = {grade['id']: grade for grade in grades}
    if not expected or len(expected) != len(cases) or len(observed) != len(grades) or expected.keys() != observed.keys():
        raise ValueError('policy case coverage mismatch')
    required_total = required_found = forbidden_total = forbidden_found = passed = 0
    for identity, case in expected.items():
        grade = observed[identity]
        for field, gold in [('required_present', 'required'), ('forbidden_present', 'forbidden')]:
            labels = grade.get(field)
            if not isinstance(labels, list) or len(labels) != len(case[gold]) or any(type(label) is not bool for label in labels):
                raise ValueError('missing or invalid policy fact labels')
        if not isinstance(grade.get('explanation'), str) or not grade['explanation'].strip():
            raise ValueError('policy grading explanation required')
        required_total += len(case['required'])
        required_found += sum(grade['required_present'])
        forbidden_total += len(case['forbidden'])
        forbidden_found += sum(grade['forbidden_present'])
        passed += all(grade['required_present']) and not any(grade['forbidden_present'])
    return {'cases': len(cases), 'passed_cases': passed, 'case_pass_rate': passed / len(cases),
            'required_facts': required_total, 'required_facts_found': required_found,
            'required_fact_recall': required_found / required_total if required_total else None,
            'forbidden_facts': forbidden_total, 'forbidden_fact_count': forbidden_found}


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
    corpus, questions, sources = [], [], []
    for sample in data:
        project = sample['sample_id']
        sources.extend(source_receipts(sample))
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
    (output / 'sources.json').write_text(json.dumps(sources, ensure_ascii=False))
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
    if len(gold) != len(questions):
        raise ValueError('duplicate question IDs')
    bundled = any('evidence' in result for result in results)
    if bundled and not all('evidence' in result for result in results):
        raise ValueError('mixed raw and extracted rankings')
    groups = {}
    for result in results:
        question = gold[result['id']]
        if bundled and (result.get('project') != question['project'] or
                        result.get('actual_mode') != result['mode']):
            raise ValueError('project mismatch or retrieval fallback')
        if question['category'] == 5 or question['invalid_evidence'] or not question['evidence']:
            continue
        for k in (5, 10, 20):
            ranked = result['evidence'] if bundled else [[identity] for identity in result['ids']]
            score = retrieval_metrics(ranked, question['evidence'], k)
            if bundled:
                # Source-level gold does not define ideal memory-level ranking.
                score.pop('ndcg')
            for split in (question['split'], 'all'):
                key = f"{result['mode']}/{split}/{k}"
                groups.setdefault(key, []).append(score)
    return {key: dict(n=len(rows), **{metric: sum(r[metric] for r in rows) / len(rows)
                                     for metric in rows[0]})
            for key, rows in sorted(groups.items())}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    policy_score = commands.add_parser('score-policy')
    policy_score.add_argument('cases')
    policy_score.add_argument('grades')
    policy_score.add_argument('output')
    audit_parser = commands.add_parser('audit-extraction')
    audit_parser.add_argument('sources')
    audit_parser.add_argument('database')
    audit_parser.add_argument('output')
    policy_parser = commands.add_parser('prepare-policy')
    policy_parser.add_argument('cases')
    policy_parser.add_argument('output')
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
    if args.command == 'score-policy':
        report = score_policy_grades(json.loads(Path(args.cases).read_text()),
                                     json.loads(Path(args.grades).read_text()))
        with Path(args.output).open('x') as output:
            json.dump(report, output, indent=2)
        print(json.dumps(report, indent=2))
    elif args.command == 'audit-extraction':
        sources = json.loads(Path(args.sources).read_text())
        report, memories = audit_extraction(sources, args.database)
        output = Path(args.output)
        output.mkdir(parents=True, exist_ok=False)
        (output / 'audit.json').write_text(json.dumps(report, indent=2))
        print(json.dumps(report, indent=2))
        if not report['complete']:
            raise SystemExit(1)
        (output / 'memories.json').write_text(json.dumps(memories, ensure_ascii=False, indent=2))
    elif args.command == 'prepare-policy':
        cases = json.loads(Path(args.cases).read_text())
        Path(args.output).write_text(json.dumps(policy_receipts(cases), ensure_ascii=False, indent=2))
    elif args.command == 'prepare':
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
