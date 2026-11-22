export const FIXTURE_ISSUER = 'https://shared-threads.test/';

export const FIXTURE_ACCOUNTS = Object.freeze({
  alice: Object.freeze({
    token: `evt_${'a'.repeat(64)}`,
    principal: Object.freeze({
      issuer: FIXTURE_ISSUER,
      subject: 'alice',
      org: 'org-alice',
    }),
  }),
  bob: Object.freeze({
    token: `evt_${'b'.repeat(64)}`,
    principal: Object.freeze({
      issuer: FIXTURE_ISSUER,
      subject: 'bob',
      org: 'org-bob',
    }),
  }),
});
