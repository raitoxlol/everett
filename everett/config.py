"""Everett's small config file: ~/.everett/config.toml (flat keys, optional [sections]).

Supported keys (all optional):

    vault = "~/Notes"                # enables `trunk view` and the core mirror
    vault_dir = "Everett"            # folder inside the vault (default "Everett")
    default_harness = "claude"       # harness for new sessions: claude | codex | omp | pi
    router = "local"                 # default router: local | jev
    typesafe_api_key = "..."         # Jev key ([jev] api_key also works)
    merge_llm = "claude"             # `trunk merge` default: claude | codex | none
"""
from __future__ import annotations

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
            end = value.find(quote, 1)
            value = value[1:end] if end > 0 else value[1:]
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
