"""Восстанавливает рынки Omni Flux CL по NFT и закрепляет отдельные размещения Bin и Vault."""
import argparse
import json
from pathlib import Path
from eth_abi import decode,encode
from eth_utils import keccak
from record_base_launch_bindings import read
from record_base_launch_hooks import DATA

TYPES=["address","address","address","address","uint24","bytes32"]
MC="0xca11bde05977b3631167028862be2a173976ca11"
def record():
    with __import__("gzip").open(DATA/"omni-v2-base.json.gz") as f:d=json.load(f)
    router=d["router"]["address"]
    at=json.loads((DATA/"base.json").read_bytes())["selector"];calls=[]
    def payload(sig,ts=(),vs=()):return keccak(text=sig)[:4]+encode(ts,vs)
    def invoke(addr,data):
        raw=read("eth_call",[dict(to=addr,data="0x"+data.hex()),at])
        calls.append(dict(to=addr,data="0x"+data.hex(),result=raw))
        return bytes.fromhex(raw[2:])
    def one(a,n,types,ts=(),vs=()):return decode(types,invoke(a,payload(n,ts,vs)))
    roles={n:one(router,n+"()",["address"])[0] for n in ["clPoolManager","binPoolManager","FLUX_CL_POSITION_MANAGER","FLUX_BIN_POSITION_MANAGER","vault"]}
    pm=roles["FLUX_CL_POSITION_MANAGER"];m=roles["clPoolManager"]
    if one(pm,"clPoolManager()",["address"])[0]!=m or one(roles["FLUX_BIN_POSITION_MANAGER"],"binPoolManager()",["address"])[0]!=roles["binPoolManager"]:raise ValueError("роли LP manager изменены")
    n,=one(pm,"nextTokenId()",["uint256"])
    def batch(qs):
        r=one(MC,"aggregate3((address,bool,bytes)[])",["(bool,bytes)[]"],["(address,bool,bytes)[]"],[[(a,True,b) for a,b in qs]])
        return r[0]
    keys={};positions={};unavailable=[]
    for first in range(1,n,40):
        inputs=list(range(first,min(first+40,n)))
        rs=batch([(pm,payload("getPoolAndPositionInfo(uint256)",["uint256"],[i])) for i in inputs])
        for i,(ok,r) in zip(inputs,rs,strict=True):
            if not ok:unavailable.append(i);continue
            key,info=decode(["("+",".join(TYPES)+")","uint256"],r)
            if not any(key[-1]) and int(key[0],16)==0 and int(key[1],16)==0:unavailable.append(i);continue
            if key[0]>=key[1] or key[3]!=m:raise ValueError(f"посторонний manager у LP {i}")
            pid="0x"+keccak(encode(TYPES,key)).hex()
            keys[pid]=key;positions[i]=dict(id=pid,info=str(info))
    markets=[]
    for pid,k in keys.items():
        v=bytes.fromhex(pid[2:])
        if one(m,"poolIdToPoolKey(bytes32)",TYPES,["bytes32"],[v])!=k:raise ValueError("неверный poolId")
        gs=one(m,"getSlot0(bytes32)",["uint160","int24","uint24","uint24"],["bytes32"],[v])
        l=one(m,"getLiquidity(bytes32)",["uint128"],["bytes32"],[v])[0]
        markets.append(dict(id=pid,key=[*k[:-1],"0x"+k[-1].hex()],slot0=list(gs),liquidity=str(l)))
    return dict(selector=at,router=router,roles=roles,next_id=n,positions=positions,unavailable=unavailable,markets=markets,calls=calls)
if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument("output",type=Path);a=parser.parse_args()
    a.output.write_text(json.dumps(record(),indent=2)+"\n",encoding="utf8")
