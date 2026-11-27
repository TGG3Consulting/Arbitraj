"""Воспроизводит привязку двух двадцаток к исходной методике, первичным контрактам и настройкам.
Запуск: python scripts/record_llama_top_bindings.py [каталог из dimension-adapters@be9967c3].
Без локальной копии загружает только нужные файлы с закреплённого GitHub SHA.
"""
import argparse
import hashlib
import json
import re
from pathlib import Path

import requests

from llama_top_scope import rows

ROOT = Path(__file__).resolve().parents[1]
DATA = ROOT / 'tests/data/first_run_dex_2026-11-26'
REV = 'be9967c3f0d3ccd06c871bab0d16beb3304de719'
EXTRA = {
    ('base','4524'): ['0x9592cd9b267748cbfbde90ac9f7df3c437a6d51b'],
    ('base','5919'): ['0xc7264db7c78dd418632b73a415595c7930a9eea4'],
    ('base','7984'): ['0x1dc4a586009dfc09c30749f0dd8e34210cd49184','0x16c31c7bada26242cf5d8f0485d1b2e708c804a1','0xc2be7b94d5498b1aceb914f3a1dec4ce502d235f'],
    ('base','8180'): ['0xc81e2084a8e5aac41ddf1fa0b9203aa81f332fa5'],
    ('ethereum','6650'): ['0x2c158bc456e027b2affccadf1bdbd9f5fc4c5c8c'],
    ('ethereum','7984'): ['0x054f0377e07d2f460151f935dffc4d880017e63a'],
    ('ethereum','9436'): ['0x77047af6cd8f7f84d96a020a2833d24916b75fde'],
    ('ethereum','7979'): ['0xb1076fe3ab5e28005c7c323bac5ac06a680d452e','0x5979458912f80b96d30d4220af8e2e4925a33320','0xf6d5b2fefec7b420d9990b95790adfd3d4c4f977'],
    ('ethereum','2116'): ['0x00000000000004533fe15556b1e086bb1a72ceae','0xdef1c0ded9bec7f1a1670819833240f027b25eff'],
}
def source_data(path, local):
    if local:
        return (local/path).read_bytes()
    url=f'https://raw.githubusercontent.com/DefiLlama/dimension-adapters/{REV}/{path}'
    response=requests.get(url, timeout=20)
    response.raise_for_status()
    return response.content

def extract(raw, ranges):
    lines=raw.decode().splitlines()
    result=[]
    for entry in ranges.split(','):
        limits=list(map(int,entry.split('-')))
        start,end=limits[0],limits[-1]
        if start<1 or start>end or start>len(lines):
            raise ValueError((entry,len(lines)))
        for index in range(start-1,min(end,len(lines))):
            result.append(lines[index])
    return '\n'.join(result)

def generate(local):
    source={}
    config=json.loads((ROOT/'config/first_run_dex.json').read_bytes())
    volume=json.loads((DATA/'catalog_recheck.json').read_bytes())
    result={}
    for network in ('base','ethereum'):
        p=ROOT.parent/'outputs'/f'{network}-integration-2026-11-26'/'sources'
        index={str(x['id']):x for x in json.loads((p/'llama-adapters.json').read_bytes())['rows']}
        rank=sorted(index,key=lambda id:-volume[network]['volume']['by_id'].get(id,-1))[:20]
        scope=rows(network)
        if set(scope)!=set(rank): raise ValueError((network,set(scope)^set(rank)))
        settings={x['id']:x['settings'] for x in config if x['network']==network}
        details=[]
        for i,id in enumerate(rank,1):
            row=scope[id]
            path='helpers/rainCard.ts' if id=='8180' else index[id]['path']
            if path not in source:source[path]=source_data(path,local)
            raw=source[path]
            excerpt=extract(raw,row['lines'])
            observed=set(a.lower() for a in re.findall(r'0x[0-9a-fA-F]{40}(?![0-9a-fA-F])',excerpt))
            domains=[]
            for profile in row['profiles']:
                deployment=settings[profile]
                key=next((k for k in ['dex.factory','dex.pool_manager','dex.pmm_pool','dex.ekubo_core'] if k in deployment),None)
                if not key:continue
                address=deployment[key].lower()
                domains.append(dict(profile=profile,setting=key,address=address,literal_in_adapter=address in observed))
            for address in EXTRA.get((network,id),[]):
                if address not in observed:raise ValueError((network,id,address,'missing literal'))
            if row['kind'] in ('factory','manager','router') and not any(d['literal_in_adapter'] for d in domains):
                raise ValueError((network,id,'no exact deployment'))
            if not (DATA/row['evidence']).exists():raise FileNotFoundError(row['evidence'])
            details.append(dict(rank=i,id=id,name=index[id]['name'],usd24h=volume[network]['volume']['by_id'][id],
                     path=path,source_sha256=hashlib.sha256(raw).hexdigest(),line_ranges=row['lines'],kind=row['kind'],
                     profiles=row['profiles'],domains=domains,other_source_addresses=EXTRA.get((network,id),[]),
                     description=row['boundary'],evidence=row['evidence']))
        result[network]=dict(captured=volume[network]['volume']['at'],rows=details)
    return dict(revision=REV,networks=result)

if __name__=='__main__':
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument('sources',nargs='?',type=Path)
    result=generate(ap.parse_args().sources)
    (DATA/'llama_top_bindings.json').write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
