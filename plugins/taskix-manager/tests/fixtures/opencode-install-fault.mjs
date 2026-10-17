import fs from 'node:fs/promises';
import { syncBuiltinESMExports } from 'node:module';

// Fail one publication rename; rollback must still be able to use the real API.
const rename = fs.rename;
let failed = false;
fs.rename = async (from, to) => {
    if (!failed && to === process.env.OPENCODE_TEST_FAIL_RENAME) {
        failed = true;
        throw Object.assign(new Error('Fixture config publication failed'), { code: 'EIO' });
    }
    return rename(from, to);
};
syncBuiltinESMExports();
