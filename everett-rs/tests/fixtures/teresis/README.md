# Teresis compatibility fixtures

`expected.json` and `memory.db` were generated with Python Teresis at
`556d706314b907b7c1eb23ac234c156668cf11c1` (database schema v1).
The checked-in fixtures are exercised without a Python checkout.

For the two live Python round trips, set `TERESIS_SRC` to the reference
revision's `src` directory. From `everett-rs/`, use a read-only export from
your local Teresis Git repository so the tests do not depend on its current
branch or uncommitted changes:

```sh
teresis_repo=/path/to/teresis
reference=$(mktemp -d)
git -C "$teresis_repo" archive --output="$reference/source.tar" \
  556d706314b907b7c1eb23ac234c156668cf11c1 src/teresis &&
  tar -xf "$reference/source.tar" -C "$reference" &&
  TERESIS_SRC="$reference/src" cargo test --locked --test memory --test teresis_compat
```

With `TERESIS_SRC` unset, only the two live Python round trips are skipped.
An explicitly selected source must pass the checks as-is. Newer Teresis
versions that use schema v2 are not compatible with this Everett version;
do not regenerate the schema-v1 fixtures from them.
