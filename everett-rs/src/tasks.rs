//! Shared per-project task list at `<project>/.everett/tasks.json`.
//! Agents create, claim, and close tasks so parallel sessions see one board
//! instead of duplicating work. The file is plain JSON, rewritten atomically.

use std::fs;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{EverettError, Result};
use crate::session::now;

const MAX_TASKS: usize = 500;
const MAX_TITLE: usize = 200;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub owner: String,
    pub created: f64,
    pub updated: f64,
}

#[derive(Serialize, Deserialize, Default)]
struct Board {
    #[serde(default)]
    tasks: Vec<Task>,
    // Monotonic id source; persisted so removed ids are never reused.
    #[serde(default)]
    seq: u64,
}

/// Nearest ancestor of `cwd` holding a `.git` entry (the project root), else `cwd`.
fn project_root(cwd: &str) -> PathBuf {
    let mut dir = if cwd.is_empty() {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    } else {
        PathBuf::from(cwd)
    };
    loop {
        if dir.join(".git").exists() {
            return dir;
        }
        if !dir.pop() {
            return PathBuf::from(cwd);
        }
    }
}

fn board_path(cwd: &str) -> Result<PathBuf> {
    let dir = project_root(cwd).join(".everett");
    for p in [&dir, &dir.join("tasks.json")] {
        if fs::symlink_metadata(p)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            return Err(bad(format!(
                "{} is a symlink; the task board must be a real file in this project",
                p.display()
            )));
        }
    }
    Ok(dir.join("tasks.json"))
}

/// Serializes read-modify-write across sessions sharing one board.
fn lock(path: &Path) -> Result<fs::File> {
    use fs2::FileExt;
    let lock_path = path.with_extension("lock");
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| EverettError::new(4, format!("cannot create {}: {e}", parent.display())))?;
    }
    let file = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&lock_path)
        .map_err(|e| EverettError::new(4, format!("cannot lock {}: {e}", lock_path.display())))?;
    file.lock_exclusive()
        .map_err(|e| EverettError::new(4, format!("cannot lock {}: {e}", lock_path.display())))?;
    Ok(file)
}

fn bad(msg: impl Into<String>) -> EverettError {
    EverettError::new(2, msg.into())
}

fn load(path: &Path) -> Result<Board> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| bad(format!("invalid task board {}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Board::default()),
        Err(e) => Err(EverettError::new(4, format!("cannot read {}: {e}", path.display()))),
    }
}

fn save(path: &Path, board: &Board) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| EverettError::new(4, format!("cannot create {}: {e}", parent.display())))?;
    }
    let text = serde_json::to_string_pretty(board).unwrap_or_else(|_| "{}".to_string());
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp = tempfile::Builder::new()
        .prefix("tasks-")
        .suffix(".tmp")
        .tempfile_in(parent)
        .map_err(|e| EverettError::new(4, format!("cannot write {}: {e}", path.display())))?;
    use std::io::Write;
    let mut file = tmp;
    file.write_all(format!("{text}\n").as_bytes())
        .and_then(|_| file.as_file().sync_all())
        .map_err(|e| EverettError::new(4, format!("cannot write {}: {e}", path.display())))?;
    file.persist(path)
        .map(|_| ())
        .map_err(|e| EverettError::new(4, format!("cannot write {}: {}", path.display(), e.error)))
}

fn find<'a>(board: &'a mut Board, id: &str) -> Result<&'a mut Task> {
    board
        .tasks
        .iter_mut()
        .find(|t| t.id == id)
        .ok_or_else(|| bad(format!("no task {id:?} on this board")))
}

fn next_id(board: &mut Board) -> String {
    loop {
        board.seq += 1;
        let id = format!("t{}", board.seq);
        if !board.tasks.iter().any(|t| t.id == id) {
            return id;
        }
    }
}

fn view(t: &Task) -> Value {
    json!({
        "id": t.id,
        "title": t.title,
        "status": t.status,
        "owner": t.owner,
        "created": t.created,
        "updated": t.updated,
    })
}

/// `everett_tasks` handler. `cwd` is the caller's working directory; `owner`
/// is the host-bound session id when Everett could detect one.
pub fn call(
    action: &str,
    cwd: &str,
    id: &str,
    title: &str,
    owner: &str,
) -> Result<Value> {
    let path = board_path(cwd)?;
    if action == "list" {
        // Saves replace the board atomically, so readers can use a snapshot.
        let board = load(&path)?;
        let tasks: Vec<Value> = board.tasks.iter().map(view).collect();
        return Ok(json!({"board": path.display().to_string(), "tasks": tasks}));
    }

    let _guard = lock(&path)?;
    let mut board = load(&path)?;
    match action {
        "add" => {
            let title = title.trim();
            if title.is_empty() {
                return Err(bad("\"title\" is required for add"));
            }
            if board.tasks.len() >= MAX_TASKS {
                return Err(bad(format!("task board is full ({MAX_TASKS} tasks); remove old tasks first")));
            }
            let at = now();
            let task = Task {
                id: next_id(&mut board),
                title: title.chars().take(MAX_TITLE).collect(),
                status: "open".into(),
                owner: String::new(),
                created: at,
                updated: at,
            };
            board.tasks.push(task);
            save(&path, &board)?;
            Ok(json!({"added": view(board.tasks.last().unwrap())}))
        }
        "claim" | "done" | "remove" => {
            if id.is_empty() {
                return Err(bad(format!("\"id\" is required for {action}")));
            }
            match action {
                "claim" => {
                    if owner.is_empty() {
                        return Err(bad(
                            "claim needs an identified caller session (bind EVERETT_SESSION_ID)",
                        ));
                    }
                    let t = find(&mut board, id)?;
                    if t.status != "open" {
                        return Err(bad(format!(
                            "cannot claim {id:?}: status is {:?}",
                            t.status
                        )));
                    }
                    t.status = "claimed".into();
                    t.owner = owner.to_string();
                    t.updated = now();
                }
                "done" => {
                    let t = find(&mut board, id)?;
                    t.status = "done".into();
                    t.updated = now();
                }
                _ => {
                    let before = board.tasks.len();
                    board.tasks.retain(|t| t.id != id);
                    if board.tasks.len() == before {
                        return Err(bad(format!("no task {id:?} on this board")));
                    }
                }
            }
            save(&path, &board)?;
            let task = board.tasks.iter().find(|t| t.id == id).map(view);
            Ok(json!({"task": task.unwrap_or(Value::Null)}))
        }
        _ => Err(bad(format!(
            "unknown action {action:?}; use list, add, claim, done, or remove"
        ))),
    }
}
