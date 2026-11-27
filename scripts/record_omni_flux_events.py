"""Читает оба события Initialize постранично. Завершённые канонические диапазоны сохраняются для продолжения."""
import argparse
import json
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import requests
from eth_utils import keccak
from record_base_launch_hooks import DATA

FIRST, LAST = 32873799, 62262756
PAGE = 10000
ENDPOINT = "https://base.api.pocket.network"
TOPICS = ["0x"+keccak(text="Initialize(bytes32,address,address,address,uint24,bytes32,"+tail+")").hex()
          for tail in ("uint160,int24","uint24")]

def scan(folder, stop=LAST):
    proof=json.loads((DATA/"omni_flux_roles.json").read_bytes())
    roles=proof["roles"]
    managers=[roles["clPoolManager"],roles["binPoolManager"]]
    folder.mkdir(parents=True,exist_ok=True)
    def get(method,params):
        for attempt in range(5):
            response=requests.post(ENDPOINT,json={"jsonrpc":"2.0","id":1,"method":method,"params":params},timeout=35)
            if response.status_code==429:
                time.sleep(max(5,int(response.headers.get("Retry-After","10"))));continue
            response.raise_for_status()
            data=response.json()
            if "result" in data:return data["result"]
            if attempt==4:raise ValueError(str(data["error"])[:300])
            time.sleep(2**attempt)
        raise ValueError("провайдер ограничил историческое чтение")
    if get("eth_getBlockByNumber",[hex(LAST),False])["hash"]!=proof["selector"]["blockHash"]:
        raise ValueError("не совпал контрольный канонический блок")
    def page(start):
        end=min(start+PAGE-1,stop);path=folder/f"{start}-{end}.json"
        if path.exists():return
        args={"address":managers,"fromBlock":hex(start),"toBlock":hex(end),"topics":[TOPICS]}
        logs=get("eth_getLogs",[args])
        seen=set()
        for log in logs:
            point=(log["blockHash"],log["logIndex"])
            if (log["address"].lower() not in managers or log["removed"] or
                not start<=int(log["blockNumber"],16)<=end or point in seen or
                log["topics"][0].lower()!=TOPICS[managers.index(log["address"].lower())]):
                raise ValueError("неверный ответ по диапазону")
            seen.add(point)
        path.with_suffix(".tmp").write_text(json.dumps(dict(endpoint=ENDPOINT,start=start,end=end,logs=logs)),encoding="utf8")
        path.with_suffix(".tmp").replace(path)
    with ThreadPoolExecutor(max_workers=2) as pool:
        list(pool.map(page,range(FIRST,stop+1,PAGE)))

if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__);p.add_argument("folder",type=Path)
    p.add_argument("--stop",type=int,default=LAST);o=p.parse_args()
    scan(o.folder,o.stop)
