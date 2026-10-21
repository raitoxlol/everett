import { cloudflareTest } from '@cloudflare/vitest-plugin';
import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: { include: ['test/relay.worker.ts'] },
  plugins: [cloudflareTest({ wrangler: { configPath: './wrangler.jsonc' } })],
});
