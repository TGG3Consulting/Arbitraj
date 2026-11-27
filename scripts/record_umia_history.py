"""Сверяет историю выдачи и миграции spot vault Umia, включая ликвидность старых физических ключей."""
import gzip
import json
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from time import sleep

import requests
from eth_abi import encode
from eth_utils import keccak
from record_omni_flux_events import ENDPOINT
from record_umia_base import AT, BLOCK, DATA, HUB, KEY, call, many, read

SIGS=["SpotLiquidityVaultRegistered(address,address)","SpotLiquidityVaultMigrated(address,address,address)"]
TOPICS=["0x"+keccak(text=s).hex() for s in SIGS]


def logs(start,end):
    p={"fromBlock":hex(start),"toBlock":hex(end),"address":HUB,"topics":[TOPICS]}
    for attempt in range(4):
        r=requests.post(ENDPOINT,json={"jsonrpc":"2.0","id":1,"method":"eth_getLogs","params":[p]},timeout=25)
        if r.ok and "result" in r.json():return r.json()["result"]
        sleep(1+attempt)
    raise ValueError(f"нет подтверждённой страницы {start}-{end}")


def capture():
    d=json.loads(gzip.decompress((DATA/"umia_base_directory.json.gz").read_bytes()))
    lo,hi=0,BLOCK
    while hi-lo>1:
        mid=(lo+hi)//2
        if read("eth_getCode",[HUB,hex(mid)])!="0x":hi=mid
        else:lo=mid
    starts=list(range(hi,BLOCK+1,100000))
    with ThreadPoolExecutor(4) as ex:raw=[l for page in ex.map(lambda a:logs(a,min(a+99999,BLOCK)),starts) for l in page]
    historic=set()
    for l in raw:
        if read("eth_getBlockByNumber",[l["blockNumber"],False])["hash"]!=l["blockHash"]:raise ValueError("событие вне канона")
        for word in l["topics"][2:]:historic.add("0x"+word[-40:])
    current={row["vault"] for row in d["rows"] if "key" in row}
    if not current.issubset(historic):raise ValueError("история не включает текущие vault")
    old=[]
    for v in sorted(historic-current):
        k,=many([call(v,"getPoolKey()",KEY)])
        p=keccak(encode([KEY],[k]))
        liq,slot=many([call(d["state_view"],"getLiquidity(bytes32)","uint128",p),call(d["state_view"],"getSlot0(bytes32)","(uint160,int24,uint24,uint24)",p)])
        old.append(dict(vault=v,key=k,id="0x"+p.hex(),liquidity=str(liq),slot0=[str(slot[0]),*slot[1:]]))
    return dict(selector=AT,endpoint=ENDPOINT,first=hi,last=BLOCK,pages=len(starts),page_size=100000,logs=raw,retired=old)


if __name__=="__main__":
    (DATA/"umia_base_history.json.gz").write_bytes(gzip.compress(json.dumps(capture(),separators=(",",":")).encode(),mtime=0))
