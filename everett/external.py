from __future__ import annotations

import argparse
import json
import math
import os
import re
import tempfile
import time
from dataclasses import asdict, dataclass
from pathlib import Path

from .inbox import HUMAN
from .session import home

HARNESSES = ('openai-dot', 'grok-bot')
RESERVED_IDS = (HUMAN, 'live')
_ID = re.compile(r'[A-Za-z0-9_-]{1,100}')


def is_external(harness: str) -> bool:
    return harness in HARNESSES


def valid_id(session_id: object) -> bool:
    return (isinstance(session_id, str) and bool(_ID.fullmatch(session_id))
            and session_id.casefold() not in RESERVED_IDS)


@dataclass(frozen=True)
class Registration:
    id: str
    harness: str
    title: str
    cwd: str
    updated: float

    @classmethod
    def from_dict(cls, record: dict) -> Registration:
        if not isinstance(record, dict) or not valid_id(record.get('id')):
            raise ValueError('External id must use 1-100 ASCII letters, digits, underscores or hyphens; '
                             f'{", ".join(RESERVED_IDS)} are reserved.')
        if not isinstance(record.get('harness'), str) or not is_external(record['harness']):
            raise ValueError(f'External harness must be one of {", ".join(HARNESSES)}.')
        if not isinstance(record.get('title'), str) or not isinstance(record.get('cwd'), str):
            raise ValueError('External title and cwd must be strings.')
        updated = record.get('updated')
        if (not isinstance(updated, (int, float)) or isinstance(updated, bool)
                or updated < 0):
            raise ValueError('External updated must be a finite non-negative epoch timestamp.')
        try:
            updated = float(updated)
        except OverflowError as error:
            raise ValueError('External updated must be a finite non-negative epoch timestamp.') from error
        if not math.isfinite(updated):
            raise ValueError('External updated must be a finite non-negative epoch timestamp.')
        return cls(record['id'], record['harness'], record['title'], record['cwd'], updated)


def directory() -> Path:
    return home() / '.everett' / 'external'


def path(session_id: str) -> Path:
    if not valid_id(session_id):
        raise ValueError(f'Invalid or reserved external id: {session_id!r}')
    return directory() / f'{session_id}.json'


def load(file: Path) -> Registration:
    if file.is_symlink() or not file.is_file():
        raise ValueError('External registration must be a regular file.')
    record = Registration.from_dict(json.loads(file.read_text(encoding='utf-8')))
    if file.stem != record.id:
        raise ValueError('External registration filename must match its id.')
    return record


def records() -> list[Registration]:
    found = []
    for file in sorted(directory().glob('*.json')):
        try:
            found.append(load(file))
        except (OSError, ValueError):
            continue
    return found


def register(session_id: str, harness: str, title: str = '', cwd: str = '',
             replace: bool = False) -> Registration:
    record = Registration.from_dict({'id': session_id, 'harness': harness, 'title': title,
                                     'cwd': cwd, 'updated': time.time()})
    target = path(record.id)
    if replace:
        previous = load(target)
        if previous.harness != record.harness:
            raise ValueError('Remove the existing registration before changing its harness.')
    target.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    with tempfile.NamedTemporaryFile(mode='w', encoding='utf-8', dir=target.parent,
                                     prefix='.registration-', delete=False) as file:
        temporary = Path(file.name)
        try:
            json.dump(asdict(record), file, ensure_ascii=False, allow_nan=False)
            file.write('\n')
            file.flush()
            os.fsync(file.fileno())
            if replace:
                os.replace(temporary, target)
            else:
                os.link(temporary, target)
        finally:
            temporary.unlink(missing_ok=True)
    return record


def remove(session_id: str) -> None:
    path(session_id).unlink()


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description='Register owner-authorized inbox-only external agents.')
    commands = parser.add_subparsers(dest='command', required=True)
    add = commands.add_parser('add')
    add.add_argument('--id', required=True, help='Logical inbox id; prefer an ext- prefix.')
    add.add_argument('--harness', required=True, choices=HARNESSES)
    add.add_argument('--title', default='')
    add.add_argument('--cwd', default='')
    add.add_argument('--replace', action='store_true', help='Update an existing registration of the same harness.')
    commands.add_parser('list')
    delete = commands.add_parser('remove')
    delete.add_argument('--id', required=True)
    args = parser.parse_args(argv)
    try:
        if args.command == 'add':
            record = register(args.id, args.harness, args.title, args.cwd, args.replace)
            print(json.dumps(asdict(record), ensure_ascii=False))
        elif args.command == 'remove':
            remove(args.id)
            print(f'Removed registration {args.id}. Its inbox is retained.')
        else:
            print(json.dumps([asdict(record) for record in records()], ensure_ascii=False))
    except (OSError, ValueError) as error:
        parser.exit(2, f'everett.external: {error}\n')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
