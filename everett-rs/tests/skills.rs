mod common;

use common::{fixture, mcp_calls, mcp_exchange, tool_json, Fixture};
use serde_json::json;

const HILL_CLIMB: &str = "---\nname: hill-climb\ndescription: Reach any goal a command can score, in small steps.\n---\n\n# Hill climb\n\nKeep wins, undo losses.\n";
const FULL_SEND: &str = "---\nname: full-send\ndescription: Trigger this if the user says 'full send'.\n---\n\n# Full send\n\nNo half measures.\n";

fn plant(fx: &Fixture, rel: &str, content: &str) {
    fx.write(&format!("{rel}/SKILL.md"), content);
}

#[test]
fn cli_lists_and_gets_skills() {
    let fx = fixture();
    plant(&fx, ".claude/skills/hill-climb", HILL_CLIMB);
    plant(&fx, ".codex/skills/full-send", FULL_SEND);
    plant(
        &fx,
        ".agents/skills/agents-only",
        "---\nname: agents-only\ndescription: Only in .agents.\n---\n\nbody\n",
    );
    plant(
        &fx,
        ".claude/skills/hub-only",
        "---\nname: hub-only\ndescription: Unique fixture skill.\n---\n\nunique body\n",
    );

    let out = fx.run(&["skills", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", fx.stdout(&out));
    let rows: Vec<serde_json::Value> = serde_json::from_str(&fx.stdout(&out)).unwrap();
    // The CLI's cwd is the real repo, so ancestor "project" roots can surface
    // same-named real skills; assert our planted (name, source, description).
    let has = |name: &str, source: &str, desc: &str| {
        rows.iter()
            .any(|r| r["name"] == name && r["source"] == source && r["description"] == desc)
    };
    assert!(
        has(
            "hill-climb",
            "claude",
            "Reach any goal a command can score, in small steps."
        ),
        "{rows:?}"
    );
    assert!(
        has(
            "full-send",
            "codex",
            "Trigger this if the user says 'full send'."
        ),
        "{rows:?}"
    );
    assert!(has("agents-only", "agents", "Only in .agents."), "{rows:?}");

    let out = fx.run(&["skills", "hill"]);
    let text = fx.stdout(&out);
    assert!(text.contains("hill-climb"), "{text}");
    assert!(!text.contains("full-send"), "{text}");

    let out = fx.run(&["skills", "--get", "hub-only"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(fx.stdout(&out).contains("unique body"));

    let out = fx.run(&["skills", "--get", "hill-climb", "--source", "claude"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(fx.stdout(&out).contains("Keep wins, undo losses."));

    let out = fx.run(&["skills", "--get", "nonexistent"]);
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn reads_native_agent_skill_folders() {
    let fx = fixture();
    for (source, folder) in [("pi", ".pi/agent/skills"), ("omp", ".omp/agent/skills"), ("hermes", ".hermes/skills"), ("grok", ".grok/skills")] {
        plant(&fx, &format!("{folder}/hill-climb"), HILL_CLIMB);
        let out = fx.run(&["skills", "--get", "hill-climb", "--source", source]);
        assert!(out.status.success() && fx.stdout(&out).contains("Keep wins"), "{source}");
    }
}

#[test]
fn pi_profile_drives_onboarding_and_skill_lookup() {
    let fx = fixture();
    let env = [("PI_CODING_AGENT_DIR", "~/custom-pi")];
    std::fs::create_dir_all(fx.home().join("custom-pi/sessions")).unwrap();
    plant(&fx, "custom-pi/skills/hill-climb", HILL_CLIMB);
    let skipped = fx.run_stdin(&["onboard", "--no-mcp", "--no-backfill"], &"\n".repeat(12), &env);
    assert!(skipped.status.success() && fx.stdout(&skipped).contains("Apply these changes now?"));
    assert!(!fx.home().join("custom-pi/extensions/everett.ts").exists());
    let setup = fx.run_stdin(&["onboard", "--yes", "--no-backfill"], "", &env);
    assert!(setup.status.success());
    assert!(fx.home().join("custom-pi/extensions/everett.ts").is_file());
    assert!(!fx.home().join(".pi").exists());
    let skill = fx.run_stdin(&["skills", "--get", "hill-climb", "--source", "pi"], "", &env);
    assert!(skill.status.success() && fx.stdout(&skill).contains("Keep wins"));
}

#[test]
fn reads_active_hermes_profile_skills() {
    let fx = fixture();
    plant(&fx, "hermes-profile/skills/hill-climb", HILL_CLIMB);
    let profile = fx.home().join("hermes-profile");
    let out = fx.run_stdin(&["skills", "--get", "hill-climb", "--source", "hermes"], "",
        &[("HERMES_HOME", profile.to_str().unwrap())]);
    assert!(out.status.success() && fx.stdout(&out).contains("Keep wins"));
}

/// The scored cases the skill hub was built against: fetch by task, the
/// shortlist, hook delivery, duplicate collapsing and the owned-folder rule.
#[test]
fn skill_hub_eval_passes() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let out = std::process::Command::new("python3")
        .arg(root.join("tests/skill_hub_eval/run.py"))
        .arg(root.join("tests/skill_hub_eval/cases.json"))
        .env("EVERETT_EVAL_BINARY", env!("CARGO_BIN_EXE_everett"))
        .output()
        .unwrap();
    let report = String::from_utf8_lossy(&out.stdout);
    assert!(report.trim_end().ends_with("failed=0"), "{report}{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn config_dirs_add_roots_and_name_matches_rank_first() {
    let fx = fixture();
    fx.write(".everett/config.toml", "[skills]\ndirs = \"~/stack/skills\"\n");
    plant(
        &fx,
        "stack/skills/prove",
        "---\nname: prove\ndescription: Show evidence.\n---\n\nprove body\n",
    );
    plant(
        &fx,
        ".claude/skills/access",
        "---\nname: access\ndescription: Approve pairings.\n---\n\nbody\n",
    );

    let out = fx.run(&["skills", "prove", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", fx.stdout(&out));
    let rows: Vec<serde_json::Value> = serde_json::from_str(&fx.stdout(&out)).unwrap();
    assert_eq!(rows[0]["name"], "prove", "{rows:?}");
    assert_eq!(rows[0]["source"], "extra", "{rows:?}");
    assert!(rows.iter().any(|r| r["name"] == "access"), "{rows:?}");

    let out = fx.run(&["skills", "--get", "prove", "--source", "extra"]);
    assert!(fx.stdout(&out).contains("prove body"));
}

#[test]
fn cli_prefers_the_best_source_and_accepts_an_explicit_one() {
    let fx = fixture();
    plant(
        &fx,
        ".claude/skills/dup",
        "---\nname: dup\ndescription: claude copy\n---\n\nclaude body\n",
    );
    plant(
        &fx,
        ".codex/skills/dup",
        "---\nname: dup\ndescription: codex copy\n---\n\ncodex body\n",
    );

    let out = fx.run(&["skills", "--get", "dup"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(fx.stdout(&out).contains("claude body"));
    assert!(String::from_utf8_lossy(&out.stderr).contains("also in codex"));

    let out = fx.run(&["skills", "--get", "dup", "--source", "codex"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(fx.stdout(&out).contains("codex body"));
}

#[test]
fn mcp_skill_tool_lists_and_gets() {
    let fx = fixture();
    plant(&fx, ".claude/skills/hill-climb", HILL_CLIMB);
    plant(&fx, ".codex/skills/full-send", FULL_SEND);

    let results = mcp_calls(
        &fx,
        &[],
        &[],
        &[
            ("everett_skill", json!({"action": "list"})),
            (
                "everett_skill",
                json!({"action": "get", "name": "hill-climb"}),
            ),
            ("everett_skill", json!({"action": "get", "name": "nope"})),
        ],
    );
    let list = tool_json(&results[0]);
    let skills = list["skills"].as_array().unwrap();
    assert!(skills
        .iter()
        .any(|s| s["name"] == "hill-climb" && s["source"] == "claude"));
    assert!(skills
        .iter()
        .any(|s| s["name"] == "full-send" && s["source"] == "codex"));

    let got = tool_json(&results[1]);
    assert_eq!(got["name"], "hill-climb");
    assert!(got["content"].as_str().unwrap().contains("Keep wins"));
    assert!(got["files"].is_array());

    let missing = tool_json(&results[2]);
    assert!(missing["error"]
        .as_str()
        .unwrap()
        .contains("no skill named"));
}

#[test]
fn mcp_exposes_skills_as_prompts() {
    let fx = fixture();
    plant(&fx, ".claude/skills/hill-climb", HILL_CLIMB);
    plant(
        &fx,
        ".codex/skills/hill-climb",
        "---\nname: hill-climb\ndescription: codex variant\n---\n\ncodex hill climb\n",
    );

    let responses = mcp_exchange(
        &fx,
        &[],
        &[],
        &[
            ("prompts/list".to_string(), json!({})),
            (
                "prompts/get".to_string(),
                json!({"name": "hill-climb", "arguments": {"input": "make it faster"}}),
            ),
            ("prompts/get".to_string(), json!({"name": "missing-skill"})),
        ],
    );

    let init_caps = &responses; // first response is prompts/list (id 1)
    let prompts = init_caps[0]["result"]["prompts"].as_array().unwrap();
    let hc = prompts
        .iter()
        .find(|p| p["name"] == "hill-climb")
        .expect("hill-climb prompt");
    assert_eq!(
        hc["description"],
        "Reach any goal a command can score, in small steps."
    );
    assert_eq!(hc["arguments"][0]["name"], "input");
    // Same prompt name from two sources dedupes to the best source (claude).
    assert_eq!(
        prompts.iter().filter(|p| p["name"] == "hill-climb").count(),
        1
    );

    let got = &responses[1]["result"];
    let text = got["messages"][0]["content"]["text"].as_str().unwrap();
    assert!(text.contains("Keep wins, undo losses."), "{text}");
    assert!(text.contains("make it faster"), "{text}");
    assert!(!text.contains("codex hill climb"), "{text}");
    // Frontmatter is stripped from prompt bodies.
    assert!(!text.contains("name: hill-climb"), "{text}");

    assert!(responses[2]["error"].is_object(), "{}", responses[2]);
}

#[test]
fn scan_covers_global_plugin_and_project_roots() {
    let home = common::lib_home();
    let root = home.path();
    let proj = root.join("proj");
    std::fs::create_dir_all(&proj).unwrap();

    let write = |rel: &str, content: &str| {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    };
    write(
        ".claude/skills/g-one/SKILL.md",
        "---\nname: g-one\ndescription: global\n---\n\nbody\n",
    );
    write(
        "proj/.agents/skills/p-one/SKILL.md",
        "# P One\n\nproject skill, no frontmatter\n",
    );
    write(
        ".local/share/devin/cli/plugins_cache/cache/src/hash/skills/plug-one/SKILL.md",
        "---\nname: plug-one\ndescription: >-\n  folded description\n  over two lines\n---\n\nplugin body\n",
    );
    write(
        ".hermes/hermes-agent/optional-skills/cat/herm-one/SKILL.md",
        "---\nname: herm-one\n---\n\nhermes body\n",
    );
    std::env::set_var("EVERETT_SKILL_DIRS", root.join("extra"));
    write(
        "extra/env-one/SKILL.md",
        "---\nname: env-one\n---\n\nenv body\n",
    );

    let skills = everett::skills::scan(&proj);
    let by_name = |n: &str| skills.iter().find(|s| s.name == n);

    assert_eq!(by_name("g-one").unwrap().source, "claude");
    assert_eq!(by_name("p-one").unwrap().source, "project");
    assert_eq!(by_name("plug-one").unwrap().source, "devin-plugins");
    assert_eq!(by_name("herm-one").unwrap().source, "hermes");
    assert_eq!(by_name("env-one").unwrap().source, "extra");

    // No frontmatter: dir name is the skill name; headings are skipped for
    // the fallback description.
    let p = by_name("p-one").unwrap();
    assert_eq!(p.description, "project skill, no frontmatter");

    // Folded block scalar joins continuation lines.
    let plug = by_name("plug-one").unwrap();
    assert_eq!(plug.description, "folded description over two lines");

    std::env::remove_var("EVERETT_SKILL_DIRS");
}
