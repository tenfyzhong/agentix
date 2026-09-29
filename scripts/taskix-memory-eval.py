#!/usr/bin/env python3
"""Read-only, reproducible Jev memory calibration against frozen reviewer labels.

prepare exports legacy backfill chunks; label them before run; report replays
cached responses, never calls providers. Raw artifacts may contain private text.
"""
import argparse
import concurrent.futures
import hashlib
import json
import math
import os
from pathlib import Path
import random
import re
import sqlite3
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
THRESHOLDS = [n / 100 for n in range(95, 49, -5)]


def encoded(value):
    return json.dumps(value, ensure_ascii=False, separators=(',', ':')).encode()


def digest(value):
    return hashlib.sha256(encoded(value)).hexdigest()


def write(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + '\n')
    path.chmod(0o600)


def chunks(text):
    raw = text.encode()
    offset = 0
    while offset < len(raw):
        end = min(offset + 16384, len(raw))
        while True:
            try:
                chunk = raw[offset:end].decode()
                break
            except UnicodeDecodeError:
                end -= 1
        yield chunk, offset, end if end < len(raw) else None
        offset = end


def skip(answer, threshold):
    if not isinstance(answer, dict):
        return False
    keys = {'skip', 'extract', 'uncertain'}
    p = answer.get('probabilities', {})
    confidence = answer.get('confidence')
    valid_number = lambda v: type(v) in (int, float) and math.isfinite(v) and 0 <= v <= 1
    if (answer.get('type') != 'choice' or answer.get('choice') not in keys
            or not valid_number(confidence) or not isinstance(p, dict)
            or set(p) != keys or not all(valid_number(v) for v in p.values())
            or abs(sum(p.values()) - 1) > .001):
        return False
    return (answer['choice'] == 'skip' and confidence >= threshold
            and p['skip'] >= threshold
            and p['skip'] - max(p['extract'], p['uncertain']) >= .2)


def recommend(rows):
    safe = [r for r in rows if r['false_skip'] == 0 and r['true_skip'] > 0]
    return max(safe, key=lambda r: (r['true_skip'], r['threshold']))['threshold'] if safe else None


def prepare(args):
    db = sqlite3.connect(f'file:{Path(args.db).expanduser()}?mode=ro', uri=True)
    jobs = [json.loads(r[0]) for r in db.execute('SELECT data FROM jobs WHERE project_id=? ORDER BY id', (args.project,))]
    db.close()
    pool = []; seen = set()
    for job in jobs:
        if job['id'] in args.exclude_job:
            continue
        for message in job.get('conversation', []):
            text = message.get('text', '')
            if message.get('role') not in ('user', 'assistant') or not text.strip():
                continue
            # Collapse repeated captured messages, not distinct discussion context.
            signature = (message['role'], text)
            if signature in seen:
                continue
            seen.add(signature)
            for text_chunk, offset, end in chunks(text):
                pool.append({'job_id': job['id'], 'message_id': message['id'],
                             'project_id': args.project,
                             'messages': [{'id': message['id'], 'role': message['role'], 'text': text}],
                             'chunk': {'message_id': message['id'], 'role': message['role'],
                                       'text': text_chunk, 'offset': offset, 'next_offset': end}})
    job_ids = sorted({r['job_id'] for r in pool})
    rng = random.Random(args.seed); rng.shuffle(job_ids)
    calibration = set(job_ids[:len(job_ids)//2])
    selected = []
    for split in ('calibration', 'holdout'):
        subset = [r for r in pool if (r['job_id'] in calibration) == (split == 'calibration')]
        rng.shuffle(subset)
        for row in subset[:args.count // 2]:
            row.update(id=f's{len(selected)+1:03}', split=split)
            selected.append(row)
    args.output.mkdir(parents=True, exist_ok=True, mode=0o700)
    write(args.output / 'samples.json', selected)
    write(args.output / 'manifest.json', {'seed': args.seed, 'jobs': len(jobs), 'unique_chunks': len(pool),
          'sample_count': len(selected), 'sample_hash': digest(selected), 'excluded_jobs': args.exclude_job,
          'mode': 'legacy_backfill_one_message_source', 'created_at': time.time()})
    print(json.dumps({'samples': len(selected), 'unique_chunks': len(pool), 'sample_bytes': len(encoded(selected))}))


def questions():
    source = (ROOT / 'crates/taskix/src/memory/triage.rs').read_text()
    # Fail if the production request shape changes rather than silently drift.
    match = re.search(r'"questions":(\{"memory_triage":.*?\})\}\)\.to_string\(\)', source)
    if not match:
        raise ValueError('Cannot locate production Jev question')
    return json.loads(match[1])


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def run(args):
    samples = json.loads((args.output / 'samples.json').read_text())
    labels = json.loads((args.output / 'labels.json').read_text())
    assert set(labels) == {s['id'] for s in samples}, 'Complete blind labels before calling Jev'
    assert all(v['label'] in ('extract', 'skip') and v['reason'] for v in labels.values())
    url = os.environ['TASKIX_JEV_URL']; key = os.environ['TASKIX_JEV_API_KEY']
    model = os.environ.get('TASKIX_JEV_MODEL', '').strip() or 'jev-latest'
    question = questions()
    write(args.output / 'question.json', question)
    def call(sample):
        body = encoded({'model': model, 'state': {'project_id': sample['project_id'],
            'receipt_id': 'eval:' + sample['id'], 'source_revision': 1,
            'messages': sample['messages'], 'chunk': sample['chunk']}, 'questions': question})
        result = {'id': sample['id'], 'body_bytes': len(body), 'answer': {}, 'error': None}
        start = time.monotonic()
        if len(body) > 30000:
            result['error'] = 'context_too_large'
        else:
            try:
                request = urllib.request.Request(url, body, {'Authorization': 'Bearer ' + key.strip(), 'Content-Type': 'application/json'})
                with urllib.request.build_opener(NoRedirect).open(request, timeout=8) as response:
                    data = response.read(1024 * 1024 + 1)
                    if len(data) > 1024 * 1024:
                        raise ValueError('response too large')
                    result['answer'] = json.loads(data).get('answers', {}).get('memory_triage', {})
            except Exception as error:
                # Never persist headers, endpoint credentials or provider error bodies.
                result['error'] = type(error).__name__
        result['duration_ms'] = (time.monotonic() - start) * 1000
        return result
    results = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
        for result in pool.map(call, samples):
            results.append(result)
            write(args.output / 'results.json', results)
            if len(results) % 10 == 0:
                print(f'Completed {len(results)}/{len(samples)}', flush=True)
    write(args.output / 'run.json', {'model': model, 'sample_hash': digest(samples), 'label_hash': digest(labels),
          'question_hash': digest(question), 'results_hash': digest(results), 'created_at': time.time(),
          'gate': 'negative_skip',
          'protocol': 'production question and score gates; urllib 8-second socket timeout, 4 HTTP workers'})


def report(args):
    samples = json.loads((args.output / 'samples.json').read_text())
    labels = json.loads((args.output / 'labels.json').read_text())
    responses = json.loads((args.output / 'results.json').read_text())
    run_info = json.loads((args.output / 'run.json').read_text())
    assert run_info.get('gate', 'negative_skip') == 'negative_skip', 'Report requires negative-gate results; preserve positive experiment reports'
    assert run_info['sample_hash'] == digest(samples) and run_info['label_hash'] == digest(labels)
    assert run_info['results_hash'] == digest(responses)
    results = {r['id']: r for r in responses}
    assert set(results) == set(labels) == {s['id'] for s in samples}
    reports = {}
    for split in ('calibration', 'holdout', 'fresh', 'all'):
        subset = [s for s in samples if split == 'all' or s['split'] == split]
        rows = []
        for threshold in THRESHOLDS:
            false = []; true = 0
            for s in subset:
                r = results[s['id']]
                if not r['error'] and skip(r['answer'], threshold):
                    if labels[s['id']]['label'] == 'skip':
                        true += 1
                    else:
                        false.append(s['id'])
            rows.append({'threshold': threshold, 'total': len(subset),
                         'gold_skip': sum(labels[s['id']]['label'] == 'skip' for s in subset),
                         'true_skip': true, 'false_skip': len(false), 'false_skip_ids': false,
                         'retained_extract': sum(labels[s['id']]['label'] == 'extract' for s in subset) - len(false),
                         'model_calls': len(subset) - true - len(false)})
        reports[split] = rows
    reports['recommended_from_calibration'] = recommend(reports['calibration'])
    reports['errors'] = {k: sum(r['error'] == k for r in responses) for k in {r['error'] for r in responses} if k}
    write(args.output / 'report.json', reports)
    print(json.dumps(reports, ensure_ascii=False, indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['prepare', 'run', 'report'])
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--db', default='~/.local/share/taskix/tasks.sqlite3')
    parser.add_argument('--project')
    parser.add_argument('--exclude-job', action='append', default=[])
    parser.add_argument('--count', type=int, default=100)
    parser.add_argument('--seed', type=int, default=20260929)
    args = parser.parse_args()
    {'prepare': prepare, 'run': run, 'report': report}[args.command](args)


if __name__ == '__main__':
    main()
