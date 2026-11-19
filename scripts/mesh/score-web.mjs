import { spawnSync } from 'node:child_process';
const r=spawnSync(process.execPath,['--test','--test-reporter=tap','web-mesh/test/console.test.mjs'],{encoding:'utf8'});
const failed=r.stdout.match(/# fail (\d+)/), total=r.stdout.match(/# tests (\d+)/);
process.stderr.write(r.stdout);
if(r.error || r.signal || !failed || !total || Number(total[1])!==7)process.exit(2);
console.log('unmet='+failed[1]);
