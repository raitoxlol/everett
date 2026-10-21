import { cloudflareTest } from '@cloudflare/vitest-plugin';
import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: { include: ['test/hosted-auth.worker.ts'] },
  plugins: [cloudflareTest({
    wrangler: { configPath: './wrangler.jsonc' },
    miniflare: { bindings: {
      EVERETT_ISSUER: 'https://everett.test',
      AUTH_GITHUB_ID: 'synthetic-client',
      AUTH_GITHUB_SECRET: 'synthetic-test-value-only',
      EVERETT_OWNER_GITHUB_ID: '282002453',
      EVERETT_OWNER_GITHUB_LOGIN: 'raitoxlol',
      EVERETT_OAUTH_REDIRECT_URIS: 'https://chatgpt.com/connector_platform_oauth_redirect',
      EVERETT_OAUTH_RESOURCES: 'https://gateway.example/mcp',
      EVERETT_OAUTH_CLIENT_IDS: 'https://chatgpt.com/oauth/client.json',
      EVERETT_OAUTH_INTROSPECTION_SECRET: 'synthetic-introspection-test-value-only',
    } },
  })],
});
