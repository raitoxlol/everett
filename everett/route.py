from __future__ import annotations

import json
import math
import os
import re
import shlex
import ssl
import time
import urllib.request

from . import config
from .session import Session, home

JEV_URL = 'https://api.typesafe.ai/v1/systemone'
JEV_MODEL = 'jev-1.13.0'
MIN_CONFIDENCE = 0.6
STOP_WORDS = frozenset('a an and are as at be by for from how i in is it of on or our the this to we with'.split())
INSTRUCTIONS = (
    'An instruction arrived for a developer who runs many parallel coding-agent sessions. '
    'Pick the one session whose ongoing work this instruction continues. '
    'Choose "new" if it is clearly a new, separate task. '
    'Choose "none" if it is ambiguous between sessions or too vague to place.'
)


def _ssl_context() -> ssl.SSLContext:
    try:
        import certifi
        return ssl.create_default_context(cafile=certifi.where())
    except ImportError:
        pass
    if os.path.exists('/etc/ssl/cert.pem'):
        return ssl.create_default_context(cafile='/etc/ssl/cert.pem')
    return ssl.create_default_context()


def find_api_key() -> str:
    """Jev key sources, in order: TYPESAFE_API_KEY, ~/.everett/config.toml, ~/.hermes/.env."""
    key = config.get('typesafe_api_key', env='TYPESAFE_API_KEY')
    if key:
        return key
    try:
        with (home() / '.hermes' / '.env').open(encoding='utf-8') as env_file:
            for line in env_file:
                name, separator, value = line.partition('=')
                if separator and name.strip() == 'TYPESAFE_API_KEY':
                    return value.strip().strip('"\'')
    except OSError:
        pass
    return ''


class RouteError(Exception):
    def __init__(self, code: int, message: str):
        super().__init__(message)
        self.code = code


def describe(session: Session) -> str:
    if session.card:
        return f'[{session.harness}] {session.cwd} — {session.card}'
    headline = session.title or session.first_user[:100]
    return f'[{session.harness}] {session.cwd} — {headline} — last: {session.last_user[:160]}'


def criteria(sessions: list[Session]) -> tuple[dict[str, str], dict[str, Session]]:
    options = {f's{i}': session for i, session in enumerate(sessions)}
    crit = {key: describe(session) for key, session in options.items()}
    crit['new'] = 'Belongs to no existing session; start a new session.'
    crit['none'] = 'Ambiguous between sessions or too vague to place.'
    return crit, options


def resume_command(session: Session, text: str) -> str:
    cd = f'cd {shlex.quote(session.cwd or ".")}'
    if session.harness == 'claude':
        return f'{cd} && claude --resume {shlex.quote(session.id)} {shlex.quote(text)}'
    if session.harness == 'codex':
        return f'{cd} && codex resume {shlex.quote(session.id)} {shlex.quote(text)}'
    if session.harness == 'pi':
        return f'{cd} && pi --session {shlex.quote(session.path)}   # then paste the text'
    if session.harness == 'hermes':
        return (f'{cd} && hermes -p {shlex.quote(session.profile or "default")} chat --resume '
                f'{shlex.quote(session.id)}   # then paste the text')
    if session.harness == 'grok':
        return f'{cd} && grok --resume {shlex.quote(session.id)}   # then paste the text'
    return f'{cd} && omp -r {shlex.quote(session.id)}   # then paste the text'


def default_harness() -> str:
    return config.get('default_harness', env='EVERETT_HARNESS', default='claude')


def new_command(text: str) -> str:
    """Manual command for starting a fresh interactive session with this request."""
    harness = default_harness()
    return f'{shlex.quote(harness)} {shlex.quote(text)}'


def call_jev(text: str, crit: dict[str, str], key: str) -> dict:
    payload = {
        'model': JEV_MODEL,
        'state': json.dumps({'incoming_instruction': text}),
        'questions': {'route': {'type': 'choice', 'instructions': INSTRUCTIONS, 'criteria': crit}},
    }
    req = urllib.request.Request(
        JEV_URL, data=json.dumps(payload).encode(),
        headers={'Authorization': f'Bearer {key}', 'Content-Type': 'application/json'}, method='POST')
    try:
        with urllib.request.urlopen(req, timeout=5, context=_ssl_context()) as resp:
            result = json.load(resp)
        return result['answers']['route']
    except Exception as exc:  # noqa: BLE001
        raise RouteError(2, f'Jev unavailable or returned an invalid response: {exc}') from exc


def _tokens(text: str) -> list[str]:
    return [token for token in re.findall(r'[^\W_]+', text.casefold(), flags=re.UNICODE)
            if token not in STOP_WORDS and (len(token) > 1 or not token.isascii())]


def _document(session: Session) -> str:
    project = session.cwd.rstrip('/').rsplit('/', 1)[-1]
    headline = session.title or session.first_user
    return ' '.join((session.card, headline, session.last_user, project))


def _bm25(query: list[str], document: list[str], document_frequency: dict[str, int],
          count: int, average_length: float) -> float:
    if not query or not document:
        return 0.0
    k1, b = 1.2, 0.75
    score = 0.0
    for token in set(query):
        frequency = document.count(token)
        if not frequency:
            continue
        df = document_frequency[token]
        inverse = math.log(1 + (count - df + 0.5) / (df + 0.5))
        score += inverse * frequency * (k1 + 1) / (frequency + k1 * (1 - b + b * len(document) / average_length))
    return score


def rank(text: str, sessions: list[Session], now: float | None = None) -> list[tuple[float, str, float]]:
    """[(score, option key, query coverage)] best first; empty when the request has no terms."""
    options = {f's{i}': session for i, session in enumerate(sessions)}
    query = _tokens(text)
    if not query or not sessions:
        return []
    docs = {key: _tokens(_document(session)) for key, session in options.items()}
    df = {token: sum(token in set(doc) for doc in docs.values()) for token in set(query)}
    average_length = max(1.0, sum(map(len, docs.values())) / len(docs))
    current = now if now is not None else time.time()
    ranked: list[tuple[float, str, float]] = []
    for key, session in options.items():
        raw_score = _bm25(query, docs[key], df, len(docs), average_length)
        age_hours = max(0.0, (current - session.last_active) / 3600)
        recency = 1 / (1 + age_hours / 72)
        coverage = sum(1 for token in set(query) if token in docs[key]) / len(set(query))
        ranked.append((raw_score * recency, key, coverage))
    ranked.sort(key=lambda item: (-item[0], int(item[1][1:])))
    return ranked


def best_dir(text: str, sessions: list[Session]) -> str:
    """Working directory of the best lexical match, for spawning a NEW session nearby."""
    for score, key, _ in rank(text, sessions):
        cwd = sessions[int(key[1:])].cwd
        if score > 0 and cwd and cwd != '/':
            return cwd
    return ''


def local_route(text: str, sessions: list[Session], now: float | None = None) -> dict:
    options = {f's{i}': session for i, session in enumerate(sessions)}
    if not sessions:
        return {'input': text, 'choice': 'new', 'confidence': 0.95,
                'decision': 'NEW', 'command': new_command(text)}
    ranked = rank(text, sessions, now)
    if not ranked:
        return {'input': text, 'choice': 'none', 'confidence': 0.0,
                'decision': 'ASK', 'suggested': None}
    best_score, best_key, coverage = ranked[0]
    if best_score <= 0 or coverage < 0.2:
        return {'input': text, 'choice': 'new', 'confidence': 0.72,
                'decision': 'NEW', 'command': new_command(text)}
    second_score = ranked[1][0] if len(ranked) > 1 else 0.0
    margin = (best_score - second_score) / best_score if best_score else 0.0
    # Coverage = share of request terms the best session matches; margin = its lead over the runner-up.
    # A tie (margin 0) stays below MIN_CONFIDENCE even at full coverage, so it asks.
    confidence = min(0.99, 0.25 + 0.3 * coverage + 0.45 * margin)
    if confidence < MIN_CONFIDENCE:
        return {'input': text, 'choice': 'none', 'confidence': round(confidence, 3),
                'decision': 'ASK', 'suggested': describe(options[best_key])}
    session = options[best_key]
    return {'input': text, 'choice': best_key, 'confidence': round(confidence, 3),
            'decision': 'SESSION', 'session': session.to_dict(),
            'command': resume_command(session, text)}


def _normalize(text: str, answer: dict, options: dict[str, Session]) -> dict:
    if not isinstance(answer, dict):
        raise RouteError(2, 'Jev returned an invalid response.')
    choice, confidence = answer.get('choice'), answer.get('confidence')
    if not isinstance(confidence, (int, float)) or isinstance(confidence, bool) or not math.isfinite(confidence):
        raise RouteError(2, 'Jev returned no valid confidence.')
    base = {'input': text, 'choice': choice, 'confidence': confidence}
    if choice in options and confidence >= MIN_CONFIDENCE:
        session = options[choice]
        return {**base, 'decision': 'SESSION', 'session': session.to_dict(),
                'command': resume_command(session, text)}
    if choice == 'new' and confidence >= MIN_CONFIDENCE:
        return {**base, 'decision': 'NEW', 'command': new_command(text)}
    return {**base, 'decision': 'ASK',
            'suggested': describe(options[choice]) if choice in options else None}


def route(text: str, sessions: list[Session], router: str | None = None,
          api_key: str | None = None, jev=call_jev) -> dict:
    """Route with Jev when a key exists (or router='jev'), otherwise the local BM25 router."""
    key = find_api_key() if api_key is None else api_key
    selected = router or config.get('router', env='EVERETT_ROUTER') or ('jev' if key else 'local')
    if selected == 'local':
        return {**local_route(text, sessions), 'router': 'local'}
    if selected != 'jev':
        raise RouteError(2, 'router must be "local" or "jev".')
    if not key:
        raise RouteError(3, 'Jev router requested but no key found: set TYPESAFE_API_KEY, '
                            'typesafe_api_key in ~/.everett/config.toml, or use --router local.')
    crit, options = criteria(sessions)
    return {**_normalize(text, jev(text, crit, key), options), 'router': 'jev'}
