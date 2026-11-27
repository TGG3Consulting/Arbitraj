"""Заявки Raindex V6 Base из официального индексатора. Цена, идентичность и отдельный vault
проверяются на одном EIP-1898 блоке от того же независимого адреса, который будет платить.
Используются только публичные eth_call/eth_getCode и GraphQL query.
"""
import argparse
import gzip
import json
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import requests
from eth_abi import decode, encode
from eth_utils import keccak

DATA = Path(__file__).resolve().parents[1] / "tests/data/first_run_dex_2026-11-26"
BOOK = "0xe522cb4a5fcb2eb31a52ff41a4653d85a4fd7c9d"
INDEX = "https://subgraph.api.ormilabs.com/api/public/9f4fc2fa-4a15-44f7-a7c1-66fdaa518a71/subgraphs/raindex-base/prod/gn"
SOURCE = "https://github.com/rainlanguage/rain.strategies/blob/d58fd9d89a040c0ace494f12abf5452776b106d5/settings.yaml"
ORDER = "(address,(address,address,bytes),(address,bytes32)[],(address,bytes32)[],bytes32)"
TAKE = f"({ORDER},uint256,uint256,(address,bytes32[],bytes)[])"
PAYER = "0x" + keccak(text="Arbitraj Base 8453 isolated owner 2026-11-26")[-20:].hex()
QUERY = """query($h:Bytes!,$id:Bytes!){
 orders(first:250,orderBy:id,orderDirection:asc,where:{active:true,id_gt:$id},block:{hash:$h})
 {id raindex{id} orderHash orderBytes owner inputs{vaultId token{address decimals symbol} balance}
 outputs{vaultId token{address decimals symbol} balance}}}"""


def orders(block):
    cursor = "0x"
    rows = []
    while True:
        response = requests.post(INDEX,json={"query":QUERY,"variables":{"h":block,"id":cursor}},timeout=25)
        response.raise_for_status()
        page = response.json()["data"]["orders"]
        for row in page:
            if row["id"] <= cursor:
                raise ValueError("unstable order cursor")
            cursor = row["id"]
            if row["raindex"]["id"].lower() == BOOK:
                rows.append(row)
        if len(page) < 250:
            return rows


def rpc(url, method, params):
    if method not in {"eth_chainId","eth_getBlockByNumber","eth_getCode","eth_call"}:
        raise ValueError(method)
    endpoints=dict.fromkeys((url,"https://base.drpc.org","https://base.rpc.blxrbdn.com","https://base.public.blockpi.network/v1/rpc/public"))
    reason=None
    for attempt in range(3):
        for endpoint in endpoints:
            try:
                r=requests.post(endpoint,json={"jsonrpc":"2.0","id":1,"method":method,"params":params},timeout=15)
                r.raise_for_status()
                ans=r.json()
                if "result" in ans or "revert" in ans.get("error",{}).get("message","").lower():
                    return ans
                reason=ans.get("error")
            except requests.RequestException as e:
                reason=str(e)
        time.sleep(attempt+1)
    raise ValueError((method,reason))


def data(name, types, args):
    selector = keccak(text=name+"("+",".join(types)+")")[:4]
    return "0x"+(selector+encode(types,args)).hex()


def result(call):
    return bytes.fromhex(call["result"][2:])


def parts(word):
    x = int.from_bytes(word, "big")
    mask = (1 << 224) - 1
    c = x & mask
    exp = x >> 224
    return (c if c < 1 << 223 else c - (1 << 224),
            exp if exp < 1 << 31 else exp - (1 << 32))


def capture(url, prefetched=None):
    base=json.loads((DATA/"base.json").read_bytes())
    tag=base["selector"]
    head=rpc(url,"eth_getBlockByNumber",[hex(base["block"]),False])["result"]
    if int(rpc(url,"eth_chainId",[])["result"],16)!=8453 or head["hash"]!=tag["blockHash"]:
        raise ValueError("canonical Base")
    rows=prefetched if prefetched is not None else orders(tag["blockHash"])
    address=lambda s:s.lower()
    def inspect(raw):
        blob=bytes.fromhex(raw["orderBytes"][2:])
        o=decode([ORDER],blob)[0]; digest="0x"+keccak(blob).hex()
        if digest!=raw["orderHash"] or encode([ORDER],[o])!=blob or o[0]!=address(raw["owner"]):
            raise ValueError("orderBytes/hash")
        def call(name,types,values):
            return rpc(url,"eth_call",[{"from":PAYER,"to":BOOK,"data":data(name,types,values)},tag])
        known={(v["token"]["address"].lower(),int(v["vaultId"],16)):v for v in raw["outputs"]}
        rows=[]
        for j,(output,vault) in enumerate(o[3]):
            indexed=known[(output,int.from_bytes(vault,"big"))]
            if parts(bytes.fromhex(indexed["balance"][2:]))[0]<=0:
                continue
            actual=result(call("vaultBalance2",["address","address","bytes32"],[o[0],output,vault]))
            for i,(source,_) in enumerate(o[2]):
                if source==output:continue
                q=call("quote2",[TAKE],[(o,i,j,[])])
                row={"i":i,"j":j,"pay":source,"receive":output,"vault":"0x"+actual.hex()}
                if "result" in q:
                    live,cap,ratio=decode(["bool","bytes32","bytes32"],result(q))
                    row.update(live=live,cap="0x"+cap.hex(),ratio="0x"+ratio.hex())
                else:
                    e=q["error"]
                    if e.get("code") not in (3,-32000,-32015) or "revert" not in e.get("message","").lower():
                        raise ValueError(e)
                    row["revert"]=(str(e.get("data",""))[:10],e.get("message","")[:100])
                rows.append(row)
        return {"hash":digest,"owner":o[0],"order":raw["orderBytes"],"quotes":rows} if rows else None
    with ThreadPoolExecutor(4) as pool:
        entries=[x for x in pool.map(inspect,rows) if x is not None]
    code=result(rpc(url,"eth_getCode",[BOOK,tag]))
    return {"source":SOURCE,"subgraph":INDEX,"anchor":tag,"height":base["block"],"book":BOOK,
            "runtime":"0x"+keccak(code).hex(),"payer":PAYER,"indexed":len(rows),"entries":entries}


if __name__=="__main__":
    parser=argparse.ArgumentParser()
    parser.add_argument("--rpc",default="https://base.gateway.tenderly.co")
    parser.add_argument("--prefetched",type=Path)
    parser.add_argument("--out",type=Path,required=True)
    a=parser.parse_args()
    rows=json.loads(gzip.decompress(a.prefetched.read_bytes())) if a.prefetched else None
    doc=capture(a.rpc,rows)
    a.out.write_bytes(gzip.compress(json.dumps(doc,separators=(",",":")).encode(),mtime=0))
