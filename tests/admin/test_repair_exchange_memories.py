import copy
import importlib.util
from pathlib import Path
import unittest
import io
import json
from unittest.mock import patch
from types import SimpleNamespace
import os
spec = importlib.util.spec_from_file_location('repair', Path(__file__).resolve().parents[2] / 'scripts/repair_exchange_memories.py')
repair = importlib.util.module_from_spec(spec)
spec.loader.exec_module(repair)

class Double:
    def __init__(self):
        self.rows = [dict(id='saved', workspace_id='cortex', app_id='cortex', user_id='synthetic-user',
                         memory_type='semantic', title='Old title', content='Input:\nQuestion\n\nAnswer:\nAnswer'),
                     dict(id='orphan', workspace_id='cortex', app_id='cortex', user_id='synthetic-user',
                         memory_type='semantic', title='Old title', content='Input:\nOld\n\nAnswer:\nAnswer'),
                     dict(id='file', workspace_id='cortex', app_id='cortex', user_id='synthetic-user',
                         memory_type='file', title='Synthetic file', content='Input:\nFile\n\nAnswer:\nText'),
                     dict(id='note', workspace_id='cortex', app_id='cortex', user_id='synthetic-user',
                         memory_type='semantic', title='Synthetic note', content='Keep this note'),
                     dict(id='other-app', workspace_id='cortex', app_id='other', memory_type='semantic',
                         content='Input:\nOther\n\nAnswer:\nAnswer')]
        self.links = set()
        self.writes = []
    def all_memories(self):return copy.deepcopy(self.rows)
    def context(self, memory):
        linked = memory['id'] in self.links
        return {'nodes':[{'id':'resource:chat','node_type':'chat','metadata':{'source_system':'cortex','external_id':'synthetic-session'}}] if linked else [],
                'edges':[{'source_id':memory['id'],'target_id':'resource:chat','edge_type':'created_in'}] if linked else []}
    def patch_title(self, mid, title):
        self.writes.append(('title',mid))
        next(m for m in self.rows if m['id']==mid)['title']=title
    def link(self, mid, row):self.writes.append(('link',mid));self.links.add(mid)
    def delete(self, memory):self.writes.append(('delete',memory['id']));self.rows=[m for m in self.rows if m['id']!=memory['id']]

class RepairTests(unittest.TestCase):
    def source(self):return {'rows':[{'memory_id':'saved','session_id':'synthetic-session','user_id':'synthetic-user','chat_title':'Synthetic chat','input_text':'  New\n question  '}], 'referenced_ids':['saved'],'file_memory_ids':['file']}
    def test_dry_run_never_writes(self):
        api=Double();result=repair.repair(self.source(),api,delete_orphans=True)
        self.assertEqual(api.writes,[]);self.assertEqual(result['title_changes'],1);self.assertEqual(result['orphans'],1)
    def test_apply_and_second_apply_are_idempotent(self):
        api=Double();repair.repair(self.source(),api,apply=True)
        self.assertEqual(api.writes,[('title','saved'),('link','saved')]);api.writes.clear()
        result=repair.repair(self.source(),api,apply=True)
        self.assertEqual(api.writes,[]);self.assertEqual(result['unchanged'],1)
    def test_orphans_need_explicit_flag_and_files_notes_other_apps_survive(self):
        api=Double();repair.repair(self.source(),api,apply=True)
        self.assertIn('orphan',[m['id'] for m in api.rows])
        repair.repair(self.source(),api,apply=True,delete_orphans=True)
        self.assertEqual({m['id'] for m in api.rows},{'saved','file','note','other-app'})
    def test_wrong_user_is_never_changed(self):
        api=Double();source=self.source();source['rows'][0]['user_id']='other-user'
        result=repair.repair(source,api,apply=True,delete_orphans=True)
        self.assertNotIn(('title','saved'),api.writes);self.assertEqual(result['skipped_scope'],1)
    def test_titles_match_unicode_word_boundary_and_file_fallback(self):
        self.assertEqual(repair.memory_title('  first\n second ', 'Chat'),'first second')
        self.assertEqual(repair.memory_title(' ', 'Chat'),'Chat')
        self.assertEqual(repair.memory_title('a'*75+' partialword','Chat'),'a'*75)
        self.assertEqual(len(repair.memory_title('ก'*100,'Chat')),80)

class HttpContractTests(unittest.TestCase):
    def test_resource_payload_and_read_only_list_routes(self):
        calls=[]
        def response(request,timeout):
            calls.append((request.get_method(),request.full_url,None if request.data is None else json.loads(request.data)))
            return io.BytesIO(json.dumps({'data':[] if request.get_method()=='GET' else {'id':'synthetic-resource'}}).encode())
        with patch.object(repair,'urlopen',response):
            api=repair.Hippo('https://api.example.test')
            api.all_memories()
            api.link('saved',{'session_id':'synthetic-session','chat_title':'Synthetic title'})
        self.assertEqual(calls[0][0],'GET')
        self.assertIn('workspace_id=cortex',calls[0][1])
        self.assertIn('app_id=cortex',calls[0][1])
        self.assertEqual(calls[1][2],{'resource_type':'chat','source_system':'cortex','external_id':'synthetic-session','title':'Synthetic title'})
        self.assertEqual(calls[2][2]['relationship'],'created_in')
        self.assertNotIn('title',calls[2][2])
    def test_sql_credentials_are_environment_only(self):
        value='postgresql://synthetic_user:synthetic_password@database.example.test/cortex_api_test'
        with patch.dict(os.environ,{'CORTEX_TEST_DATABASE_URL':value}),patch.object(repair.subprocess,'run') as run:
            run.return_value=SimpleNamespace(returncode=0,stdout='{"rows":[],"referenced_ids":[],"file_memory_ids":[]}')
            repair.source_rows(False)
            args,kwargs=run.call_args
            self.assertNotIn('synthetic_password',' '.join(args[0]))
            self.assertEqual(kwargs['env']['PGPASSWORD'],'synthetic_password')
            self.assertEqual(kwargs['env']['PGDATABASE'],'cortex_api_test')
    def test_report_has_ids_and_counts_without_chat_text(self):
        result=repair.repair(RepairTests().source(),Double())
        text=json.dumps(result)
        self.assertNotIn('New question',text)
        self.assertNotIn('synthetic-user',text)
        self.assertNotIn('Input:',text)

if __name__ == '__main__':unittest.main()
