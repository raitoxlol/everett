"""Trunk memory: sessions push learnings up (`learn`), a merge distills them (`trunk merge`),
and every new session starts from the same shared core (SessionStart hooks).

Layout under ~/.everett/core/:
    inbox.jsonl              pending learnings, one JSON object per line
    core.md                  global core (<= 300 words)
    projects/<project>.md    one core per project (<= 300 words each)
    history/<stamp>/         previous core files and the archived inbox of each merge
"""
from __future__ import annotations

import json
import math
import os
import re
import shutil
import subprocess
import tempfile
import time
from collections import Counter
from pathlib import Path

from . import config
from .session import home

MAX_LEARNING = 500
CORE_WORDS = 300
CONTEXT_WORDS = 350
LEARN_LINE = ('When you learn something other sessions should know (a decision, a convention, a gotcha), '
              'run `everett learn "<fact>"` (add `--project <name>` for project-only facts).')


class CoreError(Exception):
    def __init__(self, message: str, code: int = 2):
        super().__init__(message)
        self.code = code


# ---- paths ---------------------------------------------------------------------------------

def core_dir() -> Path:
    return home() / '.everett' / 'core'


def inbox_path() -> Path:
    return core_dir() / 'inbox.jsonl'


def global_path() -> Path:
    return core_dir() / 'core.md'


def project_path(project: str) -> Path:
    return core_dir() / 'projects' / f'{slug(project)}.md'


def history_dir() -> Path:
    return core_dir() / 'history'


def slug(name: str) -> str:
    return re.sub(r'[^a-z0-9._-]+', '-', (name or '').strip().casefold()).strip('-.')[:64]


def project_for(cwd: str) -> str:
    """Project name for a directory: the enclosing git repo's folder, else the folder itself.

    The home directory and / are not projects."""
    if not cwd:
        return ''
    path = Path(cwd).expanduser()
    stop = {Path('/'), home(), Path.home()}
    if path in stop:
        return ''
    probe = path
    for _ in range(12):
        if (probe / '.git').exists():
            return slug(probe.name)
        if probe.parent == probe or probe.parent in stop:
            break
        probe = probe.parent
    return slug(path.name)


# ---- secret filter -------------------------------------------------------------------------

SECRET_PATTERNS = [
    (r'-----BEGIN [A-Z ]*PRIVATE KEY-----', 'a private key'),
    (r'\b(?:sk|pk|rk)-(?:ant-|proj-|live-|test-)?[A-Za-z0-9_-]{16,}', 'an API key'),
    (r'\bgh[pousr]_[A-Za-z0-9]{20,}|\bgithub_pat_[A-Za-z0-9_]{20,}', 'a GitHub token'),
    (r'\bxox[abposr]-[A-Za-z0-9-]{10,}', 'a Slack token'),
    (r'\bAKIA[0-9A-Z]{16}\b', 'an AWS access key'),
    (r'\bAIza[0-9A-Za-z_-]{30,}', 'a Google API key'),
    (r'\beyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}', 'a JWT'),
    (r'\b[Bb]earer\s+[A-Za-z0-9._~+/=-]{16,}', 'a bearer token'),
    (r'(?i)\b(?:password|passwd|pwd|secret|api[_-]?key|access[_-]?token|auth[_-]?token|token)\b\s*[:=]\s*\S{6,}',
     'a credential assignment'),
    (r'(?i)\b[a-z][a-z0-9+.-]*://[^/\s:@]+:[^/\s@]+@', 'a URL with a password'),
]


def _entropy(token: str) -> float:
    counts = Counter(token)
    return -sum(n / len(token) * math.log2(n / len(token)) for n in counts.values())


def find_secret(text: str) -> str:
    """Return what kind of secret the text seems to contain, or ''."""
    for pattern, kind in SECRET_PATTERNS:
        if re.search(pattern, text):
            return kind
    for token in re.findall(r'[A-Za-z0-9_+/=.-]{24,}', text):
        classes = sum(bool(re.search(p, token)) for p in (r'[a-z]', r'[A-Z]', r'[0-9]'))
        if classes >= 3 and _entropy(token) >= 4.0 and not re.fullmatch(r'[a-z0-9._/-]+', token):
            return 'a high-entropy token'
    return ''


# ---- push: learn ---------------------------------------------------------------------------

def detect_harness() -> str:
    env = os.environ
    if env.get('CLAUDECODE') or env.get('CLAUDE_CODE_ENTRYPOINT'):
        return 'claude'
    if any(k.startswith('CODEX_') for k in env):
        return 'codex'
    if any(k.startswith('OMP_') for k in env):
        return 'omp'
    if any(k.startswith('HERMES_') for k in env):
        return 'hermes'
    return ''


def learn(text: str, project: str | None = None, scope: str | None = None, cwd: str | None = None) -> dict:
    """Validate and append one learning to the inbox. Raises CoreError on rejection."""
    from .send import caller_session_id
    text = ' '.join((text or '').split())
    if not text:
        raise CoreError('The learning is empty.')
    if len(text) > MAX_LEARNING:
        raise CoreError(f'The learning is {len(text)} characters; the limit is {MAX_LEARNING}. '
                        'Write the one fact other sessions need.')
    kind = find_secret(text)
    if kind:
        raise CoreError(f'Rejected: this looks like it contains {kind}. Secrets never go into the shared core.')
    cwd = cwd if cwd is not None else os.getcwd()
    scope = scope or ('project' if project else 'global')
    if scope not in ('global', 'project'):
        raise CoreError('scope must be "global" or "project".')
    project = slug(project) if project else project_for(cwd)
    if scope == 'project' and not project:
        raise CoreError('No project: pass --project, or run from inside a project folder.')
    entry = {'ts': time.time(), 'session': caller_session_id(), 'harness': detect_harness(),
             'cwd': cwd, 'project': project, 'scope': scope, 'text': text}
    inbox_path().parent.mkdir(parents=True, exist_ok=True)
    with inbox_path().open('a', encoding='utf-8') as f:
        f.write(json.dumps(entry, ensure_ascii=False) + '\n')
    return entry


def read_inbox() -> list[dict]:
    items = []
    try:
        with inbox_path().open(encoding='utf-8') as f:
            for line in f:
                try:
                    item = json.loads(line)
                except ValueError:
                    continue
                if isinstance(item, dict) and item.get('text'):
                    items.append(item)
    except FileNotFoundError:
        pass
    return items


# ---- merge ---------------------------------------------------------------------------------

def words(text: str) -> int:
    return len(text.split())


def _bullets(markdown: str) -> list[str]:
    out = []
    for line in markdown.splitlines():
        line = line.strip()
        if line.startswith(('- ', '* ')):
            out.append(line[2:].strip())
    return [b for b in out if b]


def _norm(text: str) -> str:
    return re.sub(r'[\W_]+', ' ', text.casefold()).strip()


def render(title: str, bullets: list[str], limit: int = CORE_WORDS) -> str:
    """Markdown bullets under a title, dropping the oldest bullets until it fits the word cap."""
    bullets = list(bullets)
    while bullets and words(f'# {title}\n' + '\n'.join(f'- {b}' for b in bullets)) > limit:
        bullets.pop(0)
    return f'# {title}\n\n' + ''.join(f'- {b}\n' for b in bullets)


def cap(markdown: str, limit: int = CORE_WORDS) -> str:
    """Enforce the word cap on LLM output: drop oldest bullets, else cut words."""
    if words(markdown) <= limit:
        return markdown.strip() + '\n'
    lines = markdown.strip().splitlines()
    while words('\n'.join(lines)) > limit and any(l.lstrip().startswith(('- ', '* ')) for l in lines):
        index = next(i for i, l in enumerate(lines) if l.lstrip().startswith(('- ', '* ')))
        lines.pop(index)
    text = '\n'.join(lines)
    if words(text) > limit:
        text = ' '.join(text.split()[:limit])
    return text.strip() + '\n'


def _read(path: Path) -> str:
    try:
        return path.read_text(encoding='utf-8')
    except OSError:
        return ''


def _title(project: str) -> str:
    return f'Project core: {project}' if project else 'Everett core'


def merge_none(current: dict[str, str], items: list[dict]) -> dict[str, str]:
    """Deterministic merge: append new facts, drop exact/normalized duplicates, keep newest last."""
    grouped: dict[str, list[dict]] = {}
    for item in sorted(items, key=lambda i: i.get('ts', 0)):
        key = item.get('project', '') if item.get('scope') == 'project' else ''
        grouped.setdefault(key, []).append(item)
    result = dict(current)
    for key in grouped:
        bullets = _bullets(current.get(key, ''))
        for item in grouped.get(key, []):
            text = item['text']
            bullets = [b for b in bullets if _norm(b) != _norm(text)] + [text]
        if bullets:
            result[key] = render(_title(key), bullets)
    return result


MERGE_PROMPT = '''You maintain the shared core memory for many parallel coding-agent sessions.
Merge the NEW LEARNINGS into the CURRENT CORE files.
Rules:
- Keep only durable facts other sessions need: decisions, conventions, gotchas, paths, commands.
- Deduplicate. When facts contradict, the newer one wins; note it briefly, e.g. "(was: X)".
- Drop stale, one-off, or session-specific items.
- Each file is Markdown: one "# " title line, then "- " bullets. At most {limit} words per file.
- Never include secrets, tokens, or passwords.
Reply with ONLY a JSON object, no prose and no code fence:
{{"global": "<markdown>", "projects": {{"<project>": "<markdown>"}}}}
Include every project that has a core or a new learning.

CURRENT CORE:
{current}

NEW LEARNINGS (oldest first; scope and project given):
{items}
'''


def _llm_command(llm: str, prompt: str, out_file: str) -> list[str]:
    from .send import spawn_command
    return spawn_command(llm, prompt, out_file=out_file)


def merge_llm(current: dict[str, str], items: list[dict], llm: str, timeout: float = 300,
              runner=subprocess.run) -> dict[str, str]:
    """Slow path: a headless harness run (claude -p / codex exec) merges the core."""
    shown = '\n\n'.join(f'## {("project " + k) if k else "global"}\n{v.strip()}' for k, v in current.items()) or '(empty)'
    lines = '\n'.join(f'- [{i.get("scope")}{":" + i["project"] if i.get("scope") == "project" else ""}] {i["text"]}'
                      for i in sorted(items, key=lambda i: i.get('ts', 0)))
    prompt = MERGE_PROMPT.format(limit=CORE_WORDS, current=shown, items=lines)
    with tempfile.TemporaryDirectory(prefix='everett-merge-') as work:
        out_file = os.path.join(work, 'out.txt') if llm == 'codex' else ''
        command = _llm_command(llm, prompt, out_file)
        try:
            result = runner(command, cwd=work, capture_output=True, text=True, timeout=timeout, check=False,
                            env={**os.environ, 'EVERETT_SEND': '1'})
        except (OSError, subprocess.TimeoutExpired) as exc:
            raise CoreError(f'{llm} merge failed to run: {exc}', 5) from exc
        if result.returncode:
            raise CoreError(f'{llm} merge exited {result.returncode}: {(result.stderr or "").strip()[-500:]}', 6)
        text = _read(Path(out_file)) if out_file else ''
        text = text or result.stdout or ''
    match = re.search(r'\{.*\}', text, re.S)
    try:
        data = json.loads(match.group(0)) if match else None
    except ValueError:
        data = None
    if not isinstance(data, dict) or not isinstance(data.get('global', ''), str):
        raise CoreError(f'{llm} returned no valid JSON core; nothing was changed.', 6)
    merged = {'': data.get('global', '')}
    for name, body in (data.get('projects') or {}).items():
        if isinstance(body, str) and slug(name):
            merged[slug(name)] = body
    for key, body in list(merged.items()):
        if find_secret(body):
            raise CoreError(f'{llm} output for {key or "global"} looks like it contains a secret; nothing was changed.', 6)
        merged[key] = cap(body) if body.strip() else ''
    return {k: v for k, v in merged.items() if v}


def current_core() -> dict[str, str]:
    core = {}
    if global_path().exists():
        core[''] = _read(global_path())
    for path in sorted((core_dir() / 'projects').glob('*.md')):
        core[path.stem] = _read(path)
    return core


def _write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(f'.{path.name}.tmp')
    tmp.write_text(text, encoding='utf-8')
    tmp.replace(path)


def merge(llm: str = 'none', dry_run: bool = False, runner=subprocess.run) -> dict:
    """Merge the inbox into the core. Returns {'merged': n, 'files': {...}, 'history': path|None}."""
    items = read_inbox()
    if not items:
        return {'merged': 0, 'files': {}, 'history': None}
    current = current_core()
    if llm == 'none':
        result = merge_none(current, items)
    elif llm in ('claude', 'codex'):
        result = merge_llm(current, items, llm, runner=runner)
    else:
        raise CoreError('--llm must be claude, codex, or none.')
    files = {str(global_path() if not k else project_path(k)): v for k, v in result.items()}
    if dry_run:
        return {'merged': len(items), 'files': files, 'history': None, 'dry_run': True}
    stamp = time.strftime('%Y%m%d-%H%M%S')
    snap = history_dir() / stamp
    n = 1
    while snap.exists():
        snap = history_dir() / f'{stamp}-{n}'
        n += 1
    snap.mkdir(parents=True)
    if global_path().exists():
        shutil.copy2(global_path(), snap / 'core.md')
    if (core_dir() / 'projects').is_dir():
        shutil.copytree(core_dir() / 'projects', snap / 'projects')
    for path, text in files.items():
        _write(Path(path), text)
    inbox_path().replace(snap / 'inbox.jsonl')
    mirror = write_vault_mirror(result)
    return {'merged': len(items), 'files': files, 'history': str(snap), 'mirror': str(mirror) if mirror else None}


def write_vault_mirror(result: dict[str, str]) -> Path | None:
    from .trunk import vault_folder
    folder = vault_folder()
    if folder is None:
        return None
    parts = ['---', 'title: Everett Core', 'source: everett trunk merge', 'tags: [everett, generated]', '---', '',
             'Generated by `everett trunk merge`. Do not edit here; use `everett learn`.', '']
    if '' in result:
        parts.append(result[''].strip())
    for key in sorted(k for k in result if k):
        parts += ['', result[key].strip()]
    path = folder / 'Core.md'
    _write(path, '\n'.join(parts) + '\n')
    return path


# ---- pull: context for new sessions --------------------------------------------------------

def _trim(text: str, budget: int) -> str:
    if budget <= 0:
        return ''
    tokens = text.split()
    if len(tokens) <= budget:
        return text.strip()
    kept, count = [], 0
    for line in text.strip().splitlines():
        n = words(line)
        if count + n > budget:
            break
        kept.append(line)
        count += n
    return '\n'.join(kept).strip()


def context(cwd: str = '') -> str:
    """Global core + this project's core, bounded to CONTEXT_WORDS in total, plus the learn line.

    Project facts are kept first when the budget is tight; they are the more specific ones."""
    project = project_for(cwd)
    project_text = _read(project_path(project)).strip() if project else ''
    global_text = _read(global_path()).strip()
    budget = CONTEXT_WORDS - words(LEARN_LINE) - 12
    project_text = _trim(project_text, budget)
    global_text = _trim(global_text, budget - words(project_text))
    body = '\n\n'.join(t for t in (global_text, project_text) if t)
    header = 'Everett shared core (the same for every session; facts other sessions learned):'
    return f'{header}\n{body}\n\n{LEARN_LINE}' if body else LEARN_LINE


def default_llm() -> str:
    return config.get('merge_llm', env='EVERETT_MERGE_LLM', default='claude')
