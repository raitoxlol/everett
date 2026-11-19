import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const reportDir = mkdtempSync(join(tmpdir(), 'everett-mesh-score-'));
try {
  const reportFile = join(reportDir, 'report.json');
  const result = spawnSync(process.execPath, [
    'node_modules/vitest/vitest.mjs', 'run', '--config', 'vitest.mesh.config.ts',
    '--reporter=json', '--outputFile=' + reportFile,
  ], { cwd: new URL('../../cloudflare/', import.meta.url), encoding: 'utf8' });
  process.stderr.write(result.stderr || '');
  if (result.error) throw result.error;
  if (result.status !== 0 && result.status !== 1) {
    throw new Error('Vitest did not complete');
  }

  const report = JSON.parse(readFileSync(reportFile, 'utf8'));
  const expectedTests = 9;
  if (report.numTotalTests !== expectedTests || report.numPassedTests + report.numFailedTests !== expectedTests ||
      (result.status === 0) !== (report.numFailedTests === 0)) {
    throw new Error('Incomplete or inconsistent mesh test report');
  }
  console.log('unmet=' + report.numFailedTests);
} finally {
  rmSync(reportDir, { recursive: true, force: true });
}
