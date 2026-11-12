#!/usr/bin/env python3
"""Operator-only repair for #118. Default: read-only dry run.

Credentials come from environment variables or the existing ignored .env file.
Production apply requires Cortex API to be stopped to prevent Save-stage races.
Only Hippocampus HTTP APIs change memories. SQL is used for reads only.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
from urllib.parse import urlencode, urlsplit, quote, unquote, parse_qs
from urllib.request import Request, urlopen

WORKSPACE = APP = 'cortex'


def memory_title(text, chat_title):
    text = ' '.join(text.split())
    if not text:
        return chat_title
    title = text[:80]
    if len(text) > 80 and not text[80].isspace() and ' ' in title:
        title = title.rsplit(' ', 1)[0]
    return title.rstrip()


def scope(memory):
    return memory.get('workspace_id') == WORKSPACE and memory.get('app_id') == APP


class Hippo:
    def __init__(self, base):
        parsed = urlsplit(base)
        if parsed.scheme not in ('http', 'https') or not parsed.hostname or parsed.username or parsed.password:
            raise ValueError('Use an HTTP API URL without credentials')
        self.base = base.rstrip('/')

    def call(self, method, path, body=None, query=None):
        url = self.base + '/api/v1/' + path
        if query:
            url += '?' + urlencode(query)
        request = Request(url, data=None if body is None else json.dumps(body).encode(),
                          headers={'Content-Type': 'application/json'}, method=method)
        try:
            with urlopen(request, timeout=30) as response:
                return json.load(response)['data']
        except Exception:
            raise RuntimeError('The memory API operation failed') from None

    def all_memories(self):
        # The existing list API has a limit but no offset. A single bounded SQL
        # integer limit avoids silently omitting older rows.
        return self.call('GET', 'memories', query={'workspace_id': WORKSPACE, 'app_id': APP, 'limit': 2147483647})

    def context(self, memory):
        return self.call('GET', 'graph/nodes/' + quote(memory['id'], safe=''), query={
            'workspace_id': WORKSPACE, 'app_id': APP, 'user_id': memory.get('user_id') or '', 'depth': 0, 'limit': 1})

    def patch_title(self, memory_id, title):
        self.call('PATCH', 'memories/' + quote(memory_id, safe=''), {'title': title})

    def link(self, memory_id, row):
        resource = {'resource_type': 'chat', 'source_system': APP, 'external_id': row['session_id'], 'title': row['chat_title']}
        self.call('POST', 'resources', resource)
        self.call('POST', 'memories/' + quote(memory_id, safe='') + '/resources',
                  {key: value for key, value in {**resource, 'relationship': 'created_in'}.items() if key != 'title'})

    def delete(self, memory):
        self.call('POST', 'memories/delete', {'workspace_id': WORKSPACE, 'app_id': APP,
                  'user_id': memory.get('user_id'), 'memory_ids': [memory['id']]})


def linked_chat(context, memory_id, session_id):
    chats = {n['id'] for n in context.get('nodes', []) if n.get('node_type') == 'chat'
             and n.get('metadata', {}).get('source_system') == APP
             and n.get('metadata', {}).get('external_id') == session_id}
    return any(e.get('edge_type') == 'created_in' and
               ((e.get('source_id') == memory_id and e.get('target_id') in chats) or
                (e.get('target_id') == memory_id and e.get('source_id') in chats))
               for e in context.get('edges', []))


def repair(source, api, apply=False, delete_orphans=False):
    memories = api.all_memories()
    if not isinstance(memories, list):
        raise RuntimeError('The memory list is not valid')
    by_id = {m['id']: m for m in memories}
    references = set(source.get('referenced_ids', []))
    files = set(source.get('file_memory_ids', []))
    report = dict(scanned=len(memories), missing=0, kept_files=0, skipped_scope=0,
                  skipped_unknown=0, unchanged=0, title_ids=[], link_ids=[], orphan_ids=[], deleted_ids=[])
    contexts = {}

    def context(memory):
        if memory['id'] not in contexts:
            contexts[memory['id']] = api.context(memory)
        return contexts[memory['id']]

    for row in source['rows']:
        references.add(row['memory_id'])
        memory = by_id.get(row['memory_id'])
        if memory is None:
            report['missing'] += 1
            continue
        if not scope(memory) or (memory.get('user_id') or '') != (row.get('user_id') or ''):
            report['skipped_scope'] += 1
            continue
        if memory.get('memory_type') == 'file' or memory['id'] in files:
            report['kept_files'] += 1
            continue
        title = memory_title(row['input_text'], row['chat_title'])
        title_changed = memory.get('title') != title
        link_changed = not linked_chat(context(memory), memory['id'], row['session_id'])
        if title_changed:
            report['title_ids'].append(memory['id'])
            if apply:
                api.patch_title(memory['id'], title)
        if link_changed:
            report['link_ids'].append(memory['id'])
            if apply:
                api.link(memory['id'], row)
        if not title_changed and not link_changed:
            report['unchanged'] += 1
    for memory in memories:
        if not scope(memory) or memory['id'] in references:
            continue
        if memory.get('memory_type') == 'file' or memory['id'] in files:
            report['kept_files'] += 1
            continue
        # Old exchange requests had no provenance field. Preserve every record
        # that cannot be identified by the exact old exchange envelope.
        content = memory.get('content') or ''
        if memory.get('memory_type') != 'semantic' or not content.startswith('Input:\n') or '\n\nAnswer:\n' not in content:
            report['skipped_unknown'] += 1
            continue
        if any(n.get('node_type') == 'file' for n in context(memory).get('nodes', [])):
            report['kept_files'] += 1
            continue
        report['orphan_ids'].append(memory['id'])
        if apply and delete_orphans:
            api.delete(memory)
            report['deleted_ids'].append(memory['id'])
    report.update(title_changes=len(report['title_ids']), link_changes=len(report['link_ids']),
                  orphans=len(report['orphan_ids']), deleted=len(report['deleted_ids']))
    return report


def source_rows(production):
    key = 'CORTEX_DATABASE_URL' if production else 'CORTEX_TEST_DATABASE_URL'
    value = os.environ.get(key)
    expected = 'cortex_api' if production else 'cortex_api_test'
    if not value or urlsplit(value).path.strip('/') != expected:
        raise RuntimeError('The selected database environment key is missing or not valid')
    env = os.environ.copy()
    parts = urlsplit(value)
    env['PGDATABASE'] = expected
    for key, part in [('PGHOST', parts.hostname), ('PGPORT', parts.port),
                      ('PGUSER', parts.username), ('PGPASSWORD', parts.password)]:
        if part is not None:
            env[key] = unquote(str(part))
    for key, target in [('sslmode', 'PGSSLMODE'), ('connect_timeout', 'PGCONNECT_TIMEOUT')]:
        if key in parse_qs(parts.query):
            env[target] = parse_qs(parts.query)[key][0]
    query = """SELECT json_build_object(
      'rows', COALESCE((SELECT json_agg(r) FROM (
        SELECT o.memory_id,c.session_id::text,c.user_id,c.title AS chat_title,i.text AS input_text
        FROM chat_outputs o JOIN chats c USING(session_id) JOIN chat_inputs i ON i.input_id=o.input_id
        WHERE c.app_id='cortex' AND o.memory_id IS NOT NULL ORDER BY o.output_id) r),'[]'::json),
      'referenced_ids',COALESCE((SELECT json_agg(DISTINCT memory_id) FROM chat_outputs WHERE memory_id IS NOT NULL),'[]'::json),
      'file_memory_ids',COALESCE((SELECT json_agg(DISTINCT memory_id) FROM chat_input_files),'[]'::json));"""
    result = subprocess.run(['psql', '-X', '-t', '-A', '-v', 'ON_ERROR_STOP=1'],
                            input=query, text=True, capture_output=True, env=env)
    if result.returncode:
        raise RuntimeError('The source database read failed')
    return json.loads(result.stdout)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument('--dry-run', action='store_true')
    mode.add_argument('--apply', action='store_true')
    parser.add_argument('--delete-orphans', action='store_true')
    parser.add_argument('--production', action='store_true')
    parser.add_argument('--hippocampus-url', required=True, help='API base URL without credentials')
    args = parser.parse_args(argv)
    try:
        try:
            from dotenv import dotenv_values
            for key, value in dotenv_values(Path(__file__).resolve().parents[1] / '.env').items():
                if value is not None:
                    os.environ.setdefault(key, value)
        except ImportError:
            pass  # A caller can supply the same keys through its environment.
        if args.production and args.apply:
            result = subprocess.run(['systemctl', 'is-active', '--quiet', 'cortex-api.service'], capture_output=True)
            if result.returncode != 3:
                raise RuntimeError('Stop Cortex API before production apply to prevent Save-stage races')
        result = repair(source_rows(args.production), Hippo(args.hippocampus_url), args.apply, args.delete_orphans)
        print(json.dumps(result, sort_keys=True))
        return 0
    except Exception:
        print('Repair failed. No credentials or memory text were logged.', file=sys.stderr)
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
