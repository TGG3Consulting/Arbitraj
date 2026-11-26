"""Сверяет реестр DXswapFactory, ставки всех пар и чистые выплаты обоих входов на Ethereum."""
from pathlib import Path
import argparse
from collections import Counter
import json

import requests
from eth_abi import decode
from record_balancer_markets import Capture, posts
from record_diamondswap_base import direct_wallet
from record_ethereum_v2_factories import WETH, digest, raw, registry
from record_ethereum_x7 import ENDPOINTS, ROOT_HASH, verified
from record_base_slipstream_executor import ROOT, rpc
from record_v2_chain_pools import measure

FACTORY="0xd34971bab6e5e356fd250715f5de0492bb070452"
ROUTER="0xb9960d9bca016e9748be75dd52f02188b9d0829f"
REFERENCE="0x98f29f527c8e0ecc67a3c2d5567833bee01f2a12"


def capture(endpoint, snapshot):
    cap=Capture(dict(snapshot,endpoint=endpoint))
    header=rpc(endpoint,"eth_getBlockByHash",[cap.selector["blockHash"],False])
    if header["hash"]!=cap.selector["blockHash"] or header["stateRoot"]!=ROOT_HASH or int(rpc(endpoint,"eth_chainId",[]),16)!=1:
        raise ValueError("другая сеть или блок")
    roles={}
    for name,address in (("Factory",FACTORY),("Pair",REFERENCE),("Router",ROUTER)):
        url,record,code=verified(cap,address,"DXswap"+name)
        roles[name]=dict(address=address,source=url,runtime=digest(code))
        if name=="Pair":
            init=digest(raw(record["creationBytecode"]["onchainBytecode"]))
    reported,=cap.one(FACTORY,"INIT_CODE_PAIR_HASH()",returns=["bytes32"])
    if "0x"+reported.hex()!=init or cap.one(ROUTER,"factory()",returns=["address"])!=(FACTORY,) or cap.one(ROUTER,"WETH()",returns=["address"])!=(WETH,):
        raise ValueError("другая фабрика, пара или роутер")
    markets=registry(cap,FACTORY,init)
    reads=[(r["pool"],"swapFee()",[],[]) for r in markets]
    reads.extend((t,"balanceOf(address)",["address"],[r["pool"]]) for r in markets for t in r["tokens"])
    replies=iter(posts(cap,reads))
    for row in markets:
        row["fee_bps"]=decode(["uint32"],next(replies))[0]
    for row in markets:
        row["balances"]=[str(decode(["uint256"],next(replies))[0]) for _ in row["tokens"]]
    rates=Counter(r["fee_bps"] for r in markets)
    examples=[]
    chain=json.loads((ROOT/"tests/data/base_ethereum_networks.json").read_bytes())["ethereum"]
    for fee in sorted(rates):
        funded=[r for r in markets if r["fee_bps"]==fee and WETH in r["tokens"] and r["balances"]==r["reserves"]]
        funded.sort(key=lambda r:-int(r["reserves"][r["tokens"].index(WETH)]))
        for row in funded:
            if min(map(int,row["reserves"]))==0:
                continue
            entry=dict(network="ethereum",dex="swapr_ethereum",pool=row["pool"],tokens=row["tokens"],factory=FACTORY,
                 creation_code_hash=init,runtime_hash=roles["Pair"]["runtime"],fee_fraction=[10000,fee],
                 verify_pair_boundary=True,invariant_error="DXswapPair: K")
            try:
                measured=measure(entry,chain,endpoint=endpoint)
                amounts=[int(v["in"]) for v in measured["cases"]]
                wallets=direct_wallet(cap,row,amounts,int(header["timestamp"],16),fee*100,ROUTER,wrapped=WETH)
            except (KeyError,ValueError) as exc:
                examples.append(dict(pool=row["pool"],fee_bps=fee,reason=str(exc)[:230]))
                continue
            examples.append(dict(pool=row["pool"],fee_bps=fee,measured=measured,wallets=wallets))
            break
    if {r["fee_bps"] for r in examples if r.get("wallets")}!=set(rates):
        raise ValueError(f"нет исполнения на каждой фактической ставке: {[(r['pool'],r.get('reason')) for r in examples][:5]}")
    return dict(chain_id=1,selector=cap.selector,height=snapshot["block"],state_root=ROOT_HASH,roles=roles,
                creation_code_hash=init,rates=rates,markets=markets,examples=examples,endpoint=endpoint)


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output",type=Path)
    args=parser.parse_args()
    snapshot=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/ethereum.json").read_bytes())
    errors=[]
    for endpoint in ENDPOINTS:
        try:
            result=capture(endpoint,snapshot)
            args.output.write_text(json.dumps(result,indent=2)+"\n",encoding="utf8")
            break
        except (ValueError,KeyError,RuntimeError,requests.RequestException) as exc:
            errors.append((endpoint,str(exc)[:400]))
    else:
        raise ValueError(errors)
