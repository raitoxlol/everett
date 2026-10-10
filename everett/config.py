"""Everett's small config file: ~/.everett/config.toml (flat keys, optional [sections]).

Supported keys (all optional):

    vault = "~/Notes"                # enables `trunk view` and the core mirror
    vault_dir = "Everett"            # folder inside the vault (default "Everett")
    default_harness = "claude"       # harness for new sessions: claude | codex | omp | pi | hermes | grok | devin
    router = "local"                 # default router: local | jev
    typesafe_api_key = "..."         # Jev key ([jev] api_key also works)
    merge_llm = "claude"             # `trunk merge` default: claude | codex | none
"""
from __future__ import annotations

import os
import fcntl
import os
from pathlib import Path

from .session import home

ALIASES = {'everett.vault': 'vault', 'jev.api_key': 'typesafe_api_key', 'everett.vault_dir': 'vault_dir',
           'everett.default_harness': 'default_harness', 'everett.router': 'router'}


class ConfigError(Exception):
    pass


def config_path() -> Path:
    return home() / '.everett' / 'config.toml'


def _parse(text: str) -> dict[str, str]:
    """Parse the flat subset of TOML Everett needs (strings, numbers, booleans)."""
    values: dict[str, str] = {}
    section = ''
    for raw in text.splitlines():
        line = raw.strip()
        if not line or line.startswith('#'):
            continue
        if line.startswith('[') and line.endswith(']'):
            section = line.strip('[]').strip()
            continue
        key, sep, value = line.partition('=')
        if not sep:
            continue
        value = value.strip()
        if value[:1] in ('"', "'"):
            quote = value[0]
            i = 1
            while i < len(value):
                if value[i] == '\\' and quote == '"':
                    i += 2
                    continue
                if value[i] == quote:
                    break
                i += 1
            inner = value[1:i] if i < len(value) else value[1:]
            value = _toml_unescape(inner) if quote == '"' else inner
        else:
            value = value.split('#', 1)[0].strip()
        name = f'{section}.{key.strip()}' if section else key.strip()
        values[ALIASES.get(name, name)] = value
    return values


def values() -> dict[str, str]:
    path = config_path()
    try:
        text = path.read_text(encoding='utf-8')
    except FileNotFoundError:
        return {}
    except OSError as exc:
        raise ConfigError(f'cannot read {path}: {exc}') from exc
    return _parse(text)


def get(key: str, env: str | None = None, default: str = '') -> str:
    """Environment variable first, then the config file, then the default. Never raises."""
    if env and os.environ.get(env, '').strip():
        return os.environ[env].strip()
    try:
        return values().get(key, '').strip() or default
    except ConfigError:
        return default


def _toml_escape(value: str) -> str:
    """Escape a value for a TOML basic string."""
    return (value.replace('\\', '\\\\').replace('"', '\\"').replace('\n', '\\n')
            .replace('\r', '\\r').replace('\t', '\\t'))


def _toml_unescape(value: str) -> str:
    """Inverse of _toml_escape for the escapes Everett writes."""
    out = []
    i = 0
    while i < len(value):
        if value[i] == '\\' and i + 1 < len(value):
            nxt = value[i + 1]
            simple = {'n': '\n', 'r': '\r', 't': '\t', '"': '"', '\\': '\\'}
            if nxt in simple:
                out.append(simple[nxt])
                i += 2
                continue
            if nxt == 'u' and i + 5 < len(value):
                try:
                    out.append(chr(int(value[i + 2:i + 6], 16)))
                    i += 6
                    continue
                except ValueError:
                    pass
        out.append(value[i])
        i += 1
    return ''.join(out)


def set_value(key: str, value: str) -> Path:
    """Set a top-level (no-section) key in ~/.everett/config.toml, preserving everything else.

    Creates the file (chmod 600) if it does not exist yet; if it does, its permissions are left
    as they are. Only ever touches the one line for `key`, never anything inside a `[section]`.
    """
    path = config_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    # Serialize read-modify-write across processes; only a missing file is empty.
    lock_fd = os.open(path.parent / '.config.lock', os.O_WRONLY | os.O_CREAT, 0o600)
    try:
        fcntl.flock(lock_fd, fcntl.LOCK_EX)
        try:
            text = path.read_text(encoding='utf-8')
        except FileNotFoundError:
            text = ''
        lines = text.splitlines()
        quoted = f'{key} = "{_toml_escape(value)}"'
        in_section = False
        replaced = False
        insert_at = len(lines)
        for i, raw in enumerate(lines):
            stripped = raw.strip()
            if stripped.startswith('[') and stripped.endswith(']'):
                if not in_section:
                    insert_at = i  # first section header: new top-level keys go just before it
                in_section = True
                continue
            if in_section:
                continue
            name = stripped.partition('=')[0].strip()
            if name == key:
                lines[i] = quoted
                replaced = True
                break
        if not replaced:
            lines.insert(insert_at, quoted)
        tmp = path.with_name(f'.{path.name}.{os.getpid()}.tmp')
        tmp.write_text('\n'.join(lines) + '\n', encoding='utf-8')
        tmp.chmod(0o600)
        tmp.replace(path)
        path.chmod(0o600)
        return path
    finally:
        os.close(lock_fd)
