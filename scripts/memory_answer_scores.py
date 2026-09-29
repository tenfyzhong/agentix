"""Score complete answer runs with diagnostic and optional LoCoMo-compatible metrics."""
import argparse
from collections import Counter
import json
from pathlib import Path
import re
import string


def token_f1(prediction, reference):
    def tokens(value):
        value = str(value).lower().translate(str.maketrans('', '', string.punctuation))
        return re.sub(r'\b(a|an|the)\b', ' ', value).split()
    predicted, gold = tokens(prediction), tokens(reference)
    overlap = sum((Counter(predicted) & Counter(gold)).values())
    return 2 * overlap / (len(predicted) + len(gold)) if overlap else 0.0


def locomo_score(prediction, reference, category):
    """Match the pinned LoCoMo evaluator, including its permissive list scoring."""
    if category not in (1, 2, 3, 4, 5):
        raise ValueError('unknown question category')
    if category == 5:
        return float(any(phrase in prediction.lower()
                         for phrase in ('no information available', 'not mentioned')))
    from nltk.stem import PorterStemmer
    import regex
    stemmer = PorterStemmer()

    def tokens(value):
        plain = value.lower().translate(str.maketrans('', '', string.punctuation))
        plain = regex.sub(r'\b(a|an|the|and)\b', ' ', plain)
        return [stemmer.stem(word) for word in plain.split()]

    def overlap(left, right):
        left, right = tokens(left), tokens(right)
        count = sum((Counter(left) & Counter(right)).values())
        if not count:
            return 0.0
        precision, recall = count / len(left), count / len(right)
        return 2 * precision * recall / (precision + recall)

    reference = str(reference)
    if category == 3:
        reference = reference.split(';')[0].strip()
    if category == 1:
        references = reference.split(',')
        return sum(max(overlap(part.strip(), gold.strip())
                       for part in prediction.split(',')) for gold in references) / len(references)
    return overlap(prediction, reference)


def score_answers(questions, answers, modes, grades=None, *, official_f1=False):
    questions_by_id = {q['id']: q for q in questions}
    if not questions or len(questions_by_id) != len(questions) or not modes or len(set(modes)) != len(modes):
        raise ValueError('empty or duplicate evaluation inputs')
    expected = {(identity, mode) for identity in questions_by_id for mode in modes}

    def indexed(rows):
        result = {(r['id'], r['mode']): r for r in rows}
        if len(result) != len(rows) or result.keys() != expected:
            raise ValueError('incomplete, duplicate or unexpected evaluation rows')
        return result

    answer_map = indexed(answers)
    grade_map = None if grades is None else indexed(grades)
    groups = {mode: {} for mode in modes}
    for (identity, mode), row in answer_map.items():
        question = questions_by_id[identity]
        answer = row['answer']
        if type(answer.get('abstained')) is not bool or not isinstance(answer.get('answer'), str):
            raise ValueError('invalid answer')
        category = question['category']
        if category not in (1, 2, 3, 4, 5):
            raise ValueError('unknown question category')
        values = {'abstention_rate': float(answer['abstained'])}
        if category != 5:
            if question.get('answer') is None:
                raise ValueError('missing reference answer')
            values['token_f1'] = 0.0 if answer['abstained'] else token_f1(answer['answer'], question['answer'])
        if official_f1:
            key = 'locomo_phrase_accuracy' if category == 5 else 'locomo_f1'
            values[key] = locomo_score(answer['answer'], question.get('answer'), category)
        if grade_map is not None:
            grade = grade_map[(identity, mode)]
            if any(type(grade.get(field)) is not bool for field in ('correct', 'faithful')):
                raise ValueError('semantic grades must be explicit booleans')
            values.update(semantic_accuracy=float(grade['correct']), faithfulness=float(grade['faithful']))
        partition = 'adversarial' if category == 5 else 'non_adversarial'
        for group in (partition, f'category/{category}', f'project/{question["project"]}/{partition}'):
            groups[mode].setdefault(group, []).append(values)
    return {mode: {name: {'n': len(rows), **{key: sum(r[key] for r in rows) / len(rows)
                 for key in rows[0]}} for name, rows in partitions.items()}
            for mode, partitions in groups.items()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('questions')
    parser.add_argument('output')
    parser.add_argument('--run', action='append', required=True, help='reader output directory; repeat for matched modes')
    parser.add_argument('--split', choices=['development', 'held_out', 'all'], required=True)
    parser.add_argument('--grades', help='complete JSON list of semantic labels')
    parser.add_argument('--official-f1', action='store_true', help='add LoCoMo-compatible scores; requires nltk==3.9.2')
    args = parser.parse_args()
    questions = [q for q in json.loads(Path(args.questions).read_text())
                 if args.split == 'all' or q['split'] == args.split]
    answers, modes = [], []
    for run in args.run:
        directory = Path(run)
        completion = json.loads((directory / 'completion.json').read_text())
        if completion['questions'] != len(questions):
            raise ValueError('reader completion count mismatch')
        modes.append(completion['mode'])
        answers.extend(json.loads(path.read_text()) for path in sorted(directory.glob('answer-*.json')))
    grades = None if args.grades is None else json.loads(Path(args.grades).read_text())
    report = score_answers(questions, answers, modes, grades, official_f1=args.official_f1)
    with Path(args.output).open('x') as output:
        json.dump({'metric_note': 'Diagnostic unstemmed token F1; not official LoCoMo F1',
                   'locomo_compatibility': {'enabled': args.official_f1,
                       'upstream_commit': '3eb6f2c585f5e1699204e3c3bdf7adc5c28cb376',
                       'raw_answer_text': True}, 'scores': report}, output, indent=2)
        output.write('\n')


if __name__ == '__main__':
    main()
