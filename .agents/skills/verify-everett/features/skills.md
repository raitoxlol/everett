# Skill hub

## Sub-features

Listing skills from every harness folder, fetching one skill's text, picking the one skill that fits a task, extra folders from `config.toml`, and project skills found by walking up from the working directory.

## How to get to it (user POV)

`everett skills [query] --json`, `everett skills --get <name> [--source <s>]`, `everett skills --match "<task>"`, `everett skills --shortlist`, and the `everett_skill` MCP tool.

## Driving it with the release helper

From `everett-rs/`, run `cargo test --locked --test skills`. Pass means `10 passed; 0 failed` (the count grows as cases are added). Each case builds a temporary home, plants `SKILL.md` files under harness folders (`.claude/skills`, `.agents/skills`) and under an extra folder named in `.everett/config.toml`, then runs the real binary. The test `skill_hub_eval_passes` also runs the scored cases in the repo-root `tests/skill_hub_eval/` with `python3`. It prints that report only when it fails.

By hand, from any folder. `B` is the binary under test: `everett-rs/target/release/everett` after the Drive build, or a packaged binary. The `trap` removes the temporary home even if a step fails. Run it from inside that home, see Gotchas:

```sh
B="$PWD/everett-rs/target/release/everett"   # or an absolute path to a packaged binary
h=$(mktemp -d); trap 'rm -rf "$h"' EXIT
mkdir -p "$h/.claude/skills/demo"
printf -- '---\nname: demo\ndescription: Demo skill.\n---\n\ndemo body\n' > "$h/.claude/skills/demo/SKILL.md"
cd "$h"
HOME="$h" EVERETT_HOME="$h" "$B" skills --get demo                # prints "demo body", exit 0
HOME="$h" EVERETT_HOME="$h" "$B" skills --match "use the demo skill"   # prints "demo body"; a task that fits nothing prints "no skill fits"
HOME="$h" EVERETT_HOME="$h" "$B" skills demo --json                # one row, source "claude"
```

Everett creates `$h/.everett/` as it runs. That is expected, and the `trap` removes it. The MCP tool `everett_skill` is covered by the `install` journey (see [install](install.md)).

## Gotchas

Project skills come from walking up from the working directory. If you run from a repo under your real home, the walk can reach your real `~/.agents/skills` and `~/.claude/skills`, and their copies outrank the fixture's. Always run from inside the temporary home. The test fixture does this with `current_dir`.
