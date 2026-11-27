"""Bind the Alien Base voucher to its live Carbon proxy, orders, fees and both payment forms."""
import argparse
import json
from hashlib import sha256
from pathlib import Path

import requests
from eth_abi import encode

from record_balancer_markets import Capture
from record_balancer_v1 import fingerprint,rpc
from record_base_slipstream_executor import ROOT
from record_carbon_ethereum import Market,NATIVE,collect,settle,wallet
from record_carbon_wallet import capture as protected

SOURCE="https://github.com/alienbase-xyz/docs/blob/c6cee73b54bd92104b879e6ca9a02a6d51b80276/contracts.md"
VOUCHER="0x2f3b0d35830b921fe7fcd08401c6cbbe29a72dc9"
EXPECTED_IMPL="0xe3763886bf91d5466b416b37fdf2b6337897716e"
USDC="0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"
WETH="0x4200000000000000000000000000000000000006"
IMPL_SLOT="0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc"
UINT="uint256"


def sources():
    """Check the strategy arithmetic from the actual verified source for both implementations."""
    verified={}
    for chain,implementation in ((1,"0x1e08049c180e25478bf7a0357171547b7332bf2c"),(8453,EXPECTED_IMPL)):
        url=f"https://sourcify.dev/server/v2/contract/{chain}/{implementation}"
        r=requests.get(url,params={"fields":"sources,compilation"},timeout=20)
        r.raise_for_status()
        data=r.json()
        verified[chain]={k:v["content"] for k,v in data["sources"].items() if k.endswith(".sol") and k!=".sol"}
    strategies="contracts/carbon/Strategies.sol"
    eth,base=verified[1],verified[8453]
    if eth.keys()!=base.keys() or [k for k in eth if eth[k]!=base[k]]!=[strategies]:
        raise ValueError("new source differences require review")
    expected=eth[strategies].replace("DEFAULT_TRADING_FEE_PPM = 2000; // 0.2%","DEFAULT_TRADING_FEE_PPM = 4000; // 0.4%")
    if expected!=base[strategies]:
        raise ValueError("strategy arithmetic differs")
    return {"same_sources":len(base)-1,"strategies_sha256":{str(n):sha256(d[strategies].encode()).hexdigest() for n,d in verified.items()}}


def capture(url,artifact):
    pin=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/base.json").read_bytes())
    at=pin["selector"]
    head=rpc(url,"eth_getBlockByNumber",[hex(pin["block"]),False])
    if head["hash"]!=at["blockHash"] or rpc(url,"eth_chainId",[])!="0x2105":
        raise ValueError("wrong canonical Base block")
    c=Capture(dict(pin,endpoint=url))
    proxy,=c.one(VOUCHER,"controller()",[],[],["address"])
    reply=requests.post(url,json={"jsonrpc":"2.0","id":1,"method":"eth_getStorageAt","params":[proxy,IMPL_SLOT,at]},timeout=20)
    reply.raise_for_status()
    impl="0x"+reply.json()["result"][-40:].lower()
    if impl!=EXPECTED_IMPL or impl==proxy:
        raise ValueError("unexpected controller implementation")
    empty,=c.one(impl,"pairs()",[],[],["address[2][]"])
    version,=c.one(proxy,"version()",[],[],["uint16"])
    if empty or version!=6:
        raise ValueError("implementation storage or controller version changed")
    default_fee,=c.one(proxy,"tradingFeePPM()",[],[],["uint32"])
    funding=next(p["settings"]["dex.router"] for p in json.loads((ROOT/"config/first_run_dex.json").read_bytes()) if p["id"]=="uniswap-v2-base")
    market=Market(proxy,WETH,funding)
    directory=collect(c,proxy)
    row=next(r for r in directory if set(r["tokens"])=={USDC,NATIVE})
    deadline=int(head["timestamp"],16)+600
    controls=[]
    for src,dst in [(USDC,NATIVE),(NATIVE,USDC)]:
        for by_target in (False,True):
            den=dst if by_target else src
            for amount in ((10**6,5*10**6) if den==USDC else (10**14,5*10**14)):
                trial=wallet(c,row,src,dst,amount,by_target,deadline,market)
                if trial is None:
                    raise ValueError(("no order for this side",src,dst,amount,by_target))
                controls.append(trial)
            # Ask the original controller for the combined fee once, across two real maker orders.
            eligible=[(sid,orders[tokens.index(dst)]) for sid,_,tokens,orders in row["orders"]]
            ids=[]
            name="calculateTradeSourceAmount" if by_target else "calculateTradeTargetAmount"
            part=10**5 if den==USDC else 10**13
            for sid,(y,_,a,b) in eligible:
                if y==0 or not (a or b):continue
                try:
                    qty,=c.one(proxy,name+"(address,address,(uint256,uint128)[])",["address","address","(uint256,uint128)[]"],
                               [src,dst,[(sid,part)]],["uint128"])
                except ValueError:
                    continue
                if qty>0 and (part if by_target else qty)<=y:ids.append((sid,part))
                if len(ids)==2:break
            if len(ids)==2:
                other,=c.one(proxy,name+"(address,address,(uint256,uint128)[])",["address","address","(uint256,uint128)[]"],
                             [src,dst,ids],["uint128"])
                spent,out=(other,2*part) if by_target else (2*part,other)
                controls.append(settle(c,src,dst,ids,spent,out,by_target,deadline,market))
    for r in directory:
        r["orders"]=[[str(sid),owner,tokens,[[str(v) for v in o] for o in orders]] for sid,owner,tokens,orders in r["orders"]]
    roles={role:{"address":a,**fingerprint(url,a,at)} for role,a in {"voucher":VOUCHER,"proxy":proxy,"implementation":impl,"wrapped":WETH}.items()}
    original=dict(source=SOURCE,chain_id=8453,height=pin["block"],selector=at,state_root=head["stateRoot"],roles=roles,default_fee=default_fee,
                  implementation_pair_count=len(empty),directory=directory,controls=controls,version=version)
    module=protected(url,original,artifact,market,8453,at["blockHash"])
    creation=bytes.fromhex(artifact["creation"])+encode(["address"],[proxy])
    module.update(creation_sha256=sha256(creation).hexdigest(),creation="0x"+creation.hex(),compiler=artifact["solc"])
    return {"original":original,"protected":module}


if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument("compiled",type=Path)
    p.add_argument("output",type=Path)
    args=p.parse_args()
    comparison=sources()
    errors=[]
    for url in ("https://base.drpc.org","https://base.gateway.tenderly.co","https://mainnet.base.org"):
        try:
            result=capture(url,json.loads(args.compiled.read_bytes()))
            break
        except (ValueError,RuntimeError,requests.RequestException,KeyError) as e:
            errors.append((url,str(e)[:300]))
    else:
        raise ValueError(errors)
    result["source_comparison"]=comparison
    args.output.write_bytes(json.dumps(result,separators=(",",":")).encode())
