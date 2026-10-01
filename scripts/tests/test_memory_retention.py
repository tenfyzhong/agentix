"""Opt-in semantic regression checks against a real development replay store."""
import json
import os
from pathlib import Path
import sqlite3
import unittest


@unittest.skipUnless(os.environ.get('TASKIX_BENCH_RETENTION_DB'), 'explicit development replay store required')
class HistoricalRetentionTests(unittest.TestCase):
    def test_dated_external_experience_retains_attribution_and_historical_date(self):
        path = Path(os.environ['TASKIX_BENCH_RETENTION_DB']).resolve()
        connection = sqlite3.connect(f'file:{path}?mode=ro', uri=True)
        try:
            work = connection.execute(
                "SELECT state FROM work_items WHERE project_id='conv-26' "
                "AND receipt_id='conv-26/D1:3'").fetchall()
            self.assertTrue(work, 'the development source must have been ingested')
            self.assertTrue(all(row[0] == 'done' for row in work), 'extraction must finish before checking retention')
            memories = [json.loads(row[0]) for row in connection.execute(
                "SELECT data FROM memories WHERE project_id='conv-26' AND status='active'")]
        finally:
            connection.close()
        supported = [m['content'] for m in memories if any(
            e['receipt_id'] == 'conv-26/D1:3' and e['message_id'] == 'D1:3'
            for e in m['content']['evidence'])]
        self.assertTrue(supported, 'the dated external experience was discarded')
        claims = ' '.join(m['conclusion'] for m in supported).lower()
        self.assertIn('caroline', claims)
        self.assertIn('group', claims)
        self.assertRegex(claims, r'2023-05-07|7(?:th)? may[,]? 2023|may 7(?:th)?,? 2023')
        self.assertTrue(all(m['valid_until'] is None for m in supported),
                        'a dated historical assertion must not expire as if it were current status')


if __name__ == '__main__':
    unittest.main()
