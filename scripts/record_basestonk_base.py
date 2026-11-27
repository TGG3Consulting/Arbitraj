"""Реестр BaseStonk из официального API, проверенный по PoolKey, configOf и состоянию V4."""
import argparse
import gzip
import json
from collections import Counter
from pathlib import Path

import requests

from eth_abi import decode, encode
from eth_utils import keccak
from record_eulerswap_directory import DATA, Reader
from record_stabull_base import MC, encoded

MANAGER="0x498581ff718922c3f8e6a244956af099b2652b2b"
VIEW="0xa3c0c9b65bad0b08107aa264b0f3db444b867a71"
ROUTER="0x161026041cf3701fc8c2f11d1a9121681be3718f"
V7="0x5c999109609f9f857f4d58240ccb12bf219ee044"
KEY="(address,address,uint24,int24,address)"
FEES="0x"+keccak(text="FeeTaken(bytes32,address,uint256,uint256)").hex()


def multi(r,calls):
    payload=[(target,True,encoded(signature,args)) for target,signature,args in calls]
    inp=keccak(text="aggregate3((address,bool,bytes)[])")[:4]+encode(["(address,bool,bytes)[]"],[payload])
    raw=r.rpc("eth_call",[dict(to=MC,data="0x"+inp.hex()),r.tag])
    return [b if ok else b"" for ok,b in decode(["(bool,bytes)[]"],bytes.fromhex(raw[2:]))[0]]


def capture(fee_logs):
    r=Reader("base");src=json.loads(gzip.decompress((DATA/"basestonk_base_listed.json.gz").read_bytes()))
    hooks=src["addresses"]; roles={}
    for hook in hooks:
        factory,manager=r.many([(hook,"launcher()",(),"address"),(hook,"poolManager()",(),"address")])
        assert manager==MANAGER and factory
        roles[hook]=dict(runtime=r.code_hash(hook),launcher=factory)
    assert r.many([(ROUTER,"poolManager()",(),"address")])==[MANAGER]
    first=61830757
    events=[];window_error=None if fee_logs else "not_requested"
    try:
        for a in range(first,r.height+1,10000) if fee_logs else []:
            events.extend(r.rpc("eth_getLogs",[dict(fromBlock=hex(a),toBlock=hex(min(a+9999,r.height)),address=hooks,topics=[FEES])]))
    except requests.HTTPError as err:
        if err.response.status_code not in (403,429):raise
        events=[];window_error=err.response.status_code
    counts=Counter((e["address"],e["topics"][1]) for e in events)
    markets=[];skipped=[]
    for start in range(0,len(src["launches"]),36):
        chunk=src["launches"][start:start+36];keys=[];queries=[]
        for token,pair,hook,generation in chunk:
            x,y=sorted((token,pair))
            key=(x,y,0 if hook==V7 else 3000,60,hook);pid=keccak(encode([KEY],[key]));keys.append((key,pid))
            queries.extend([(VIEW,"getSlot0(bytes32)",(pid,)),(VIEW,"getLiquidity(bytes32)",(pid,)),(hook,"configOf(bytes32)",(pid,))])
        returns=multi(r,queries)
        for i,((token,pair,hook,gen),(key,pid)) in enumerate(zip(chunk,keys)):
            slot,liquidity,config=returns[3*i:3*i+3]
            if len(slot)!=128 or len(liquidity)!=32 or not config or len(config)%32:
                skipped.append(dict(token=token,id="0x"+pid.hex(),reason="unreadable",lengths=list(map(len,(slot,liquidity,config)))));continue
            sqrt,tick,protocol,fee=decode(["uint160","int24","uint24","uint24"],slot)
            if not sqrt:
                skipped.append(dict(token=token,id="0x"+pid.hex(),reason="uninitialized"));continue
            cfg=[int.from_bytes(config[j:j+32]) for j in range(0,len(config),32)]
            if len(cfg) not in (4,9) or cfg[3 if len(cfg)==4 else 5]!=1:
                skipped.append(dict(token=token,id="0x"+pid.hex(),reason="unknown_config",config=cfg));continue
            ident="0x"+pid.hex()
            markets.append(dict(id=ident,token=token,pair=pair,hook=hook,gen=gen,key=key,sqrt=str(sqrt),tick=tick,lp=fee,protocol=protocol,
                                liquidity=str(int.from_bytes(liquidity)),config=cfg,fee_events=None if window_error else counts.get((hook,ident),0)))
    identified={m["id"] for m in markets}
    unknown=[(a,i,c) for (a,i),c in counts.items() if i not in identified]
    return dict(tag=r.tag,height=r.height,source=src["source"],manager=MANAGER,router=ROUTER,router_hash=r.code_hash(ROUTER),roles=roles,
                fee_event_window=[first,r.height],fee_events=None if window_error else len(events),window_error=window_error,unmatched_fee_events=unknown,markets=markets,skipped=skipped)


if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__);p.add_argument("output",type=Path)
    p.add_argument("--fee-logs",action="store_true",help="Требует RPC, предоставляющего исторический журнал указанных hooks")
    args=p.parse_args();args.output.write_text(json.dumps(capture(args.fee_logs),separators=(",",":")),encoding="utf8")

