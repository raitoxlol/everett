import { spawnSync } from 'node:child_process';

const result = spawnSync(process.execPath, [
  '--test', '--test-reporter=tap', 'web-mesh/test/console.test.mjs',
], { cwd: new URL('../../', import.meta.url), encoding: 'utf8' });
process.stderr.write(result.stdout || '');
process.stderr.write(result.stderr || '');
const count = name => Number(result.stdout?.match(new RegExp(`^# ${name} (\\d+)$`, 'm'))?.[1]);
const total = count('tests'), passed = count('pass'), failed = count('fail');
if (result.error || ![0, 1].includes(result.status) ||
    !Number.isInteger(total) || total <= 0 || passed + failed !== total ||
    (result.status === 0) !== (failed === 0)) process.exit(2);

console.log('unmet=' + failed);
