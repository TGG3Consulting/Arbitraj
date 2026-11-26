"""Сверяет охват каталога Homelander с уже учтёнными рынками Hydrex и QuickSwap на одном хеше."""
import hashlib
import re

import requests
from base_archive_endpoint import available
from record_baibai import Capture, FOLDER, abi, read, write

LLAMA = "https://raw.githubusercontent.com/DefiLlama/dimension-adapters/be9967c3f0d3ccd06c871bab0d16beb3304de719/"
PRIMARY = "https://raw.githubusercontent.com/mev-x-project/MEV-X-Homelander/8213de9dfe2c096aaa679c0c0f0a6ec8747c9cc0/README.md"


def capture():
    c=Capture(available(read(FOLDER/"base.json")))
    source={}
    for u in (PRIMARY,LLAMA+"dexs/homelander.ts",LLAMA+"fees/homelander/shared.ts"):
        r=requests.get(u,timeout=20);r.raise_for_status()
        source[u]=hashlib.sha256(r.content).hexdigest()
        if u.endswith("dexs/homelander.ts") and not re.search(r"doublecounted:\s*true",r.text):
            raise ValueError("Изменилась классификация агрегированного оборота")
        if u.endswith("shared.ts"):
            segment=r.text.split("[CHAIN.BASE]:",1)[1].split("factories:",1)[1].split("],",1)[0]
            registries=re.findall(r'address:\s*"(0x[0-9a-fA-F]{40})"',segment)
    if len(registries)!=2:raise ValueError("Другой набор фабрик Base")
    rows=[]
    for title in ("hydrex","quickswap"):
        d=read(FOLDER/(title+"_base.json"))
        if d["selector"]!=c.tag:raise ValueError("Иной контрольный блок")
        for market in d["markets"]:
            p=market["pool"]
            asks=[dict(to=f,data=abi("pluginByPool",["address"],[p])) for f in registries]
            asks += [dict(to=p,data=abi("plugin")),dict(to=p,data=abi("factory"))]
            resp=c.batch([("eth_call",[q,c.tag]) for q in asks])
            values=["0x"+r["result"][-40:] for r in resp]
            listed=[(reg,v) for reg,v in zip(registries,values[:2],strict=True) if int(v,16)]
            if len(listed)!=1 or values[2]!=market["plugin"]:raise ValueError("Связи плагина отличаются от первичной проверки пула")
            rows.append(dict(pool=p,dex=title,amm_factory=values[3],registry=listed[0][0],registered_plugin=listed[0][1],
                             active_plugin=values[2],matches_active=values[2]==listed[0][1],requests=asks,responses=resp))
    return dict(chain_id=8453,selector=c.tag,endpoint=c.url,sources=source,registries=registries,markets=rows)


if __name__=="__main__":
    write(FOLDER/"homelander_base.json",capture())
