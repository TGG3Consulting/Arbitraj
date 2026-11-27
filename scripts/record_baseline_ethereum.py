"""Baseline Mercury Ethereum — все опубликованные PoolId в обе стороны и их канонические V4 Initialize."""
import argparse
import gzip
import json
from pathlib import Path

from eth_abi import decode, encode
from eth_utils import keccak
import requests

from record_balancer_markets import Capture, posts
from record_base_launch_hooks import DATA, KEY
from record_base_slipstream_executor import OWNER, ROOT, rpc
from record_ethereum_v2_factories import WETH, digest, raw
from record_ethereum_x7 import ENDPOINTS, ROOT_HASH

API = "https://api.baseline.markets/v1/1inch/ethereum/pools"
INITIALIZE = digest(b"Initialize(bytes32,address,address,uint24,int24,address,uint160,int24)")

def capture(ep):
    snap = json.loads((DATA/"ethereum.json").read_bytes())
    c = Capture(dict(snap,endpoint=ep))
    head = rpc(ep, "eth_getBlockByHash", [c.selector["blockHash"], False])
    if head["stateRoot"]!=ROOT_HASH or head["hash"]!=c.selector["blockHash"] or int(head["number"],16)!=snap["block"] or rpc(ep,"eth_chainId",[])!="0x1":
        raise ValueError("wrong archive")
    entries = requests.get(API,timeout=20)
    entries.raise_for_status()
    directory=entries.json()
    relay=directory["quoteContract"].lower()
    candidates=[r for r in directory["pools"] if int(r["createdAtBlockNumber"])<=snap["block"]]
    roles={}
    def code(a):
        b=raw(rpc(ep,"eth_getCode",[a,c.selector]))
        if not b:raise ValueError("empty code at "+a)
        return dict(address=a,hash=digest(b),size=len(b))
    roles["hook"] = code(relay)
    components,=c.one(relay,"getComponents()",returns=["address[]"])
    selectors={}
    for a in components:
        label,=c.one(a,"LABEL()",returns=["bytes32"])
        label=label.rstrip(b"\0").decode()
        if label in roles:raise ValueError("duplicate component")
        roles[label]=code(a)
        routes,=c.one(a,"ROUTES()",returns=["bytes4[]"])
        answers=posts(c,[(relay,"routes(bytes4)",["bytes4"],[k]) for k in routes])
        for k,v in zip(routes,answers,strict=True):
            if decode(["address"],v)[0]!=a or k.hex() in selectors:raise ValueError("ambiguous selector")
            selectors[k.hex()]=a
    config=json.loads((ROOT/"config/first_run_dex.json").read_bytes())
    manager=next(r['settings']['dex.pool_manager'].lower() for r in config if r['chain_id']==1 and r['settings']['dex.kind']=='uniswap_v4')
    paused,=c.one(relay,"isProtocolPaused()",returns=["bool"])
    rows=[]; launch_topic=None
    for r in candidates:
        pool_id=r["uniswapPoolId"].lower()
        token=r["bToken"]["address"].lower()
        reserve=r["reserveToken"]["address"].lower()
        key,=c.one(relay,"poolKey(address)",["address"],[token],[KEY])
        bound,=c.one(relay,"poolIdToBToken(bytes32)",["bytes32"],[raw(pool_id)],["address"])
        actual,=c.one(relay,"reserve(address)",["address"],[token],["address"])
        ready,=c.one(relay,"hasHook(address)",["address"],[token],["bool"])
        locked,=c.one(relay,"isPoolPaused(address)",["address"],[token],["bool"])
        answer=c.session.post(ep,json=dict(jsonrpc="2.0",id=1,method="eth_getTransactionReceipt",params=[r["createdAtTxnId"]]),timeout=25)
        answer.raise_for_status()
        receipt=answer.json().get("result")
        if not receipt or receipt["transactionHash"].lower()!=r["createdAtTxnId"].lower():
            raise ValueError("missing registration receipt")
        init=[l for l in receipt["logs"] if l["address"].lower()==manager and l["topics"][:2]==[INITIALIZE,pool_id]]
        if len(init)!=1 or receipt["status"]!="0x1" or int(receipt["blockNumber"],16)!=int(r["createdAtBlockNumber"]):raise ValueError("different creation")
        l=init[0]
        fee,spacing,hook,_,_=decode(["uint24","int24","address","uint160","int24"],raw(l["data"]))
        event_key=(*["0x"+v[-40:] for v in l["topics"][2:]], fee,spacing,hook)
        if token!=bound or actual!=reserve or set(key[:2])!={reserve,token} or key[4]!=relay or key!=event_key or digest(encode([KEY],[key]))!=pool_id:raise ValueError("market and Initialize differ")
        # The unindexed Baseline deployment event stores the bToken address, reserve, and pool identifier in its data.
        created=[z for z in receipt["logs"] if z["address"].lower()==relay and pool_id[2:] in z["data"].lower() and token[2:] in z["data"].lower()]
        if len(created)!=1:raise ValueError("ambiguous Relay deployment")
        signature=created[0]["topics"][0]
        if launch_topic is not None and signature!=launch_topic:raise ValueError("another Relay deployment event")
        launch_topic=signature
        rows.append(dict(group="baseline-ethereum",id=pool_id,key=key,terms=dict(quote=reserve),symbol=r["bToken"]["symbol"],token=token,
                         asset=code(token),paused=locked,has_hook=ready,registration=created[0],initialize=l,sizes=[10**12,10**15]))
    if not launch_topic:raise ValueError("no verified market")
    # The HTTP API can miss a permissionless deployment. Check every emission of the attested event through H.
    response=c.session.post(ep,json=dict(jsonrpc="2.0",id=1,method="eth_getLogs",params=[dict(address=relay,topics=[launch_topic],fromBlock="0x0",toBlock=hex(snap["block"]))]),timeout=35)
    response.raise_for_status()
    canonical=response.json().get("result")
    if not isinstance(canonical,list):raise ValueError("Relay log coverage unavailable")
    listed={(r["registration"]["transactionHash"],r["registration"]["logIndex"]) for r in rows}
    discovered={(l["transactionHash"],l["logIndex"]) for l in canonical if not l.get("removed",False)}
    if len(listed)!=len(rows) or listed!=discovered:raise ValueError(f"Relay registered {len(discovered)} markets, API bound {len(listed)}")
    return dict(chain_id=1,height=snap["block"],selector=c.selector,state_root=ROOT_HASH,api=API,manager=manager,relay=relay,
                roles=roles,selectors=selectors,paused=paused,created_event=launch_topic,rows=rows,wrapped=WETH,
                holders={WETH:OWNER},fund_wrapped=True)

if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output",type=Path);parser.add_argument("--wallets",action="store_true");a=parser.parse_args()
    for ep in ENDPOINTS:
        try:
            if a.wallets:
                from record_base_launch_wallets import replay
                d=json.loads(gzip.decompress((DATA/"baseline_ethereum_directory.json.gz").read_bytes()))
                result=replay("baseline-ethereum",ep,d)
            else:result=capture(ep)
            b=(json.dumps(result,separators=(",",":"))+"\n").encode()
            a.output.write_bytes(gzip.compress(b,mtime=0) if a.output.suffix==".gz" else b);break
        except (requests.RequestException,ValueError,RuntimeError,KeyError) as exc:print(ep,str(exc)[:220],flush=True)
    else:raise ValueError("canonical market unavailable")
