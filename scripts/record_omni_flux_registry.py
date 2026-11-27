"""Соединяет первичные Initialize обеих фабрик с текущими ключами, резервами и реальными лестницами BinQuoter."""
import argparse
import gzip
import json
from pathlib import Path

from eth_abi import decode, encode
from eth_utils import keccak
from record_base_launch_bindings import read
from record_base_launch_hooks import DATA
from record_omni_flux_events import FIRST, LAST, PAGE
from record_omni_flux_bin import KEY

MC="0xca11bde05977b3631167028862be2a173976ca11"

def data(sig, types, args):
    return keccak(text=sig)[:4]+encode(types,args)

def batch(at,items):
    raw=data("aggregate3((address,bool,bytes)[])",["(address,bool,bytes)[]"],[[(a,True,calldata) for a,calldata in items]])
    result=read("eth_call",[{"to":MC,"data":"0x"+raw.hex()},at])
    answers=decode(["(bool,bytes)[]"],bytes.fromhex(result[2:]))[0]
    if len(answers)!=len(items):raise ValueError("неполный ответ Multicall3")
    return answers

def record(folder,output):
    refs=json.loads((DATA/"omni_flux_roles.json").read_bytes());at=refs["selector"];roles=refs["roles"]
    managers={roles["clPoolManager"]:"cl",roles["binPoolManager"]:"bin"}
    events=[];through=FIRST-1
    for n in range(FIRST,LAST+1,PAGE):
        last=min(n+PAGE-1,LAST);p=folder/f"{n}-{last}.json"
        if not p.is_file():break
        j=json.loads(p.read_bytes())
        if (j["start"],j["end"])!=(n,last):raise ValueError("разрыв страниц")
        events.extend(j["logs"]);through=last
    old={}
    if output.is_file():
        with gzip.open(output) as f:cached=json.load(f)
        if cached["selector"]==at:old={r["id"]:r for r in cached["markets"]}
    rows={}
    for l in events:
        pid=l["topics"][1];raw_id=bytes.fromhex(pid[2:]);m=l["address"].lower();kind=managers[m]
        if pid in rows:raise ValueError("повторная инициализация PoolId")
        if pid in old and old[pid]["initialize"]==l:
            rows[pid]=old[pid];continue
        if read("eth_getBlockByNumber",[l["blockNumber"],False])["hash"]!=l["blockHash"]:raise ValueError("Initialize вне канона")
        calls=[(m,data(s,["bytes32"],[raw_id])) for s in ("poolIdToPoolKey(bytes32)","getSlot0(bytes32)")]
        if kind=="cl":calls.append((m,data("getLiquidity(bytes32)",["bytes32"],[raw_id])))
        result=batch(at,calls)
        if not all(ok for ok,_ in result):raise ValueError("менеджер не подтвердил рынок")
        key=decode(KEY,result[0][1])
        if keccak(result[0][1])!=raw_id or key[3]!=m:raise ValueError("менеджер или keccak не совпали")
        if key[0][2:]!=l["topics"][2][-40:] or key[1][2:]!=l["topics"][3][-40:]:raise ValueError("другие валюты")
        slot=decode(["uint24"]*3 if kind=="bin" else ["uint160","int24","uint24","uint24"],result[1][1])
        row=dict(kind=kind,id=pid,key=[*key[:-1],"0x"+key[-1].hex()],slot0=list(map(str,slot)),initialize=l)
        if kind=="cl":row["liquidity"]=str(int.from_bytes(result[2][1],"big"))
        else:
            def amounts(asset):
                if asset=="0x833589fcd6edb6e08f4c7c32d4f71b54bda02913" or asset=="0xfde4c96c8593536e31f229ea8f37b2ada2699bb2":return [1,2,5,10,100,1000,10**4,10**6,10**7]
                return [1,100,10**3,10**6,10**8,10**9,10**10,10**11,10**12,10**13,10**14,10**16]
            seq=[(z,a) for z in (True,False) for a in amounts(key[0 if z else 1])]
            exact="((address,address,address,address,uint24,bytes32),bool,uint128,bytes)"
            calls=[(roles["BinQuoter"],data("quoteExactInputSingle("+exact+")",[exact],[(key,z,a,b"")])) for z,a in seq]
            answers=[]
            for i in range(0,len(calls),6):answers.extend(batch(at,calls[i:i+6]))
            row["quotes"]=[dict(forward=z,amount=str(amt),raw="0x"+v.hex(),out=str(int.from_bytes(v[:32],"big")) if ok else None) for (z,amt),(ok,v) in zip(seq,answers)]
        rows[pid]=row
    j=dict(selector=at,start=FIRST,through=through,requested_end=LAST,markets=list(rows.values()))
    temp=output.with_suffix(".tmp");temp.write_bytes(gzip.compress(json.dumps(j,indent=2).encode(),mtime=0));temp.replace(output)
if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument("folder",type=Path);p.add_argument("output",type=Path)
    args=p.parse_args();record(args.folder,args.output)
