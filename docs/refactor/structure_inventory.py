#!/usr/bin/env python3
"""Approximate local import inventory; source references, not a semantic Rust parser."""
import argparse, collections, json, re
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('root',type=Path);p.add_argument('--output',type=Path,required=True);a=p.parse_args()
edges=collections.defaultdict(set);sizes={};refs=[]
for base,pattern in [('everett-rs/src','*.rs'),('cloudflare/src','*.ts')]:
    for path in (a.root/base).rglob(pattern):
        rel=path.relative_to(a.root).as_posix();text=path.read_text();sizes[rel]=len(text.splitlines())
        if path.suffix=='.rs':
            for m in re.finditer(r'crate::([a-zA-Z_][a-zA-Z_0-9]*)',text):
                target=f'everett-rs/src/{m.group(1)}.rs'
                if not (a.root/target).exists(): target=f'everett-rs/src/{m.group(1)}/mod.rs'
                if (a.root/target).exists() and rel!=target:
                    edges[rel].add(target);refs.append({'from':rel,'to':target,'line':text.count('\n',0,m.start())+1})
        else:
            for m in re.finditer(r'from\s+[\'\"](\.[^\'\"]+)[\'\"]',text):
                target=(path.parent/m.group(1)).with_suffix('.ts').resolve()
                if target.is_file(): edges[rel].add(target.relative_to(a.root.resolve()).as_posix())
# Tarjan SCCs expose broad module cycles; references remain approximate.
index={};low={};stack=[];active=set();scc=[]
def visit(v):
    index[v]=low[v]=len(index);stack.append(v);active.add(v)
    for w in sorted(edges[v]):
        if w not in index: visit(w);low[v]=min(low[v],low[w])
        elif w in active: low[v]=min(low[v],index[w])
    if low[v]==index[v]:
        group=[]
        while True:
            w=stack.pop();active.remove(w);group.append(w)
            if w==v:break
        if len(group)>1:scc.append(sorted(group))
for v in sorted(sizes):
    if v not in index:visit(v)
reverse=collections.defaultdict(list)
for caller,targets in list(edges.items()):
    for target in targets:reverse[target].append(caller)
result={'note':__doc__,'files':len(sizes),'references':refs,'imports':{k:sorted(v) for k,v in sorted(edges.items())},'cycles':scc,'most_imported':sorted(({'path':k,'callers':len(v)} for k,v in reverse.items()),key=lambda x:-x['callers'])}
a.output.write_text(json.dumps(result,indent=2)+'\n')
print(json.dumps({k:result[k] for k in ['files','most_imported']},indent=2));print('Cyclic groups:',len(scc))
