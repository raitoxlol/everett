import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
const result = spawnSync('node', ['node_modules/vitest/vitest.mjs','run','--config','vitest.mesh.config.ts','--reporter=json','--outputFile=/tmp/everett-mesh-score.json'], { cwd: new URL('../../cloudflare/', import.meta.url), encoding:'utf8' });
process.stderr.write(result.stderr || '');
if (result.signal || result.error) process.exit(2);
let report; try { report=JSON.parse(readFileSync('/tmp/everett-mesh-score.json','utf8')); } catch { process.stderr.write(result.stdout); process.exit(2); }
if(report.numTotalTests !== 8) { process.stderr.write(JSON.stringify(report)); process.exit(2); }
console.log('unmet=' + report.numFailedTests);
