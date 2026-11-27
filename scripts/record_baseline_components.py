"""Сверяет реестр Baseline Mercury, зарегистрированные маршруты ABI и роли BSR/cbBTC."""
import argparse
import json
from pathlib import Path
from concurrent.futures import ThreadPoolExecutor
from eth_abi import decode, encode
from eth_utils import keccak
from record_base_launch_bindings import read, record as contracts
from record_base_launch_hooks import DATA, KEY

MC="0xca11bde05977b3631167028862be2a173976ca11"
def calldata(sig,types=(),args=()):
    return keccak(text=sig)[:4]+encode(types,args)

def capture():
    raw=json.loads((DATA/"base_recovered_markets.json").read_bytes())
    b=raw["baseline"]; at=raw["selector"]; relay=b["hook"]["address"]; token=b["btoken"]["address"]
    calls=[]
    def call(a,sig,returns,types=(),args=()):
        data="0x"+calldata(sig,types,args).hex()
        r=read("eth_call",[dict(to=a,data=data),at])
        calls.append(dict(contract=a,signature=sig,data=data,response=r))
        return decode(returns,bytes.fromhex(r[2:]))
    listed,=call(relay,"getComponents()",["address[]"])
    roles={}
    routes={}
    for c in listed:
        label,=call(c,"LABEL()",["bytes32"])
        name=label.rstrip(b"\0").decode()
        if name in roles:raise ValueError("duplicate component "+name)
        roles[name]=c
        selectors,=call(c,"ROUTES()",["bytes4[]"])
        batch=[(relay,False,calldata("routes(bytes4)",["bytes4"],[s])) for s in selectors]
        rows,=call(MC,"aggregate3((address,bool,bytes)[])",["(bool,bytes)[]"],["(address,bool,bytes)[]"],[batch])
        for s,(ok,result) in zip(selectors,rows,strict=True):
            target,=decode(["address"],result)
            if not ok or c!=target:raise ValueError("component route mismatch "+name+" "+s.hex())
            routes["0x"+s.hex()]=target
    known=contracts(dict(selector=at,contracts=[relay,*listed],rows=[]))
    candidate,=call(relay,"poolIdToBToken(bytes32)",["address"],["bytes32"],[bytes.fromhex(b["pool_id"][2:])])
    key,=call(relay,"poolKey(address)",[KEY],["address"],[token])
    reserve,=call(relay,"reserve(address)",["address"],["address"],[token])
    flags={name:call(relay,name,["bool"],["address"],[token])[0] for name in ("hasHook(address)","isPoolPaused(address)")}
    flags["isProtocolPaused()"]=call(relay,"isProtocolPaused()",["bool"])[0]
    if candidate!=token or list(key)!=b["key"] or reserve not in key[:2] or reserve==token:
        raise ValueError("source market differs from Initialize")
    return dict(selector=at,relay=relay,contracts=known,components=roles,routes=routes,key=key,
                pool_id=b["pool_id"],token=token,reserve=reserve,flags=flags,calls=calls,
                sdk="https://registry.npmjs.org/@baseline-markets/sdk/-/sdk-1.3.0.tgz")

if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output",type=Path);a=parser.parse_args()
    a.output.write_text(json.dumps(capture(),indent=2)+"\n",encoding="utf8")
