"""Read the complete Supernova CL directory and compare actual V5 wallet settlement against the Algebra quoter."""
import argparse
import json
from pathlib import Path

import requests
from eth_abi import decode
from eth_utils import keccak
from record_balancer_markets import Capture, posts
from record_base_slipstream_executor import OWNER
from record_hydrex_integral import EXEC, FOLDER, RUNTIME, VIRTUAL, VIRTUAL_FILE, read, rpc, simulate
from record_solidly_v3 import ETH_WETH, HOLDERS
from record_ethereum_x7 import ENDPOINTS, ROOT_HASH

FACTORY="0x44b7fbd4d87149efa5347c451e74b9fd18e89c55"
QUOTER="0xf9439cd803dcb11fa574bcc8421207f89b529e41"
REGISTRY="0xdbfd67d12cadb8925c1417ff3638693f2bf99b97"
DEPLOYER="0x45bc0f9855a626743d57d37d989f3b9462deba45"
ROUTER="0x72d63a5b080e1b89cc93f9b9f50cbfa5e291c8ac"
USDT="0xdac17f958d2ee523a2206206994597c13d831ec7"
USDC="0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"


def digest(b):
    return "0x"+keccak(b).hex()


def capture(e):
    c=Capture(e)
    tag=e["selector"]
    head=rpc(e,"eth_getBlockByHash",[tag["blockHash"],False])
    if (head["hash"]!=tag["blockHash"] or head["stateRoot"]!=ROOT_HASH
            or int(head["number"],16)!=e["block"] or int(rpc(e,"eth_chainId",[]),16)!=1):
        raise ValueError("wrong Ethereum archive")
    def address(a,s):return c.one(a,s,returns=["address"])[0]
    code=lambda a: bytes.fromhex(rpc(e,"eth_getCode",[a,tag])[2:])
    if (address(FACTORY,"defaultPluginFactory()")!=REGISTRY or address(FACTORY,"poolDeployer()")!=DEPLOYER
            or any((address(a,"factory()"),address(a,"poolDeployer()"))!=(FACTORY,DEPLOYER) for a in [QUOTER,ROUTER])):
        raise ValueError("unexpected deployment topology")
    init=c.one(FACTORY,"POOL_INIT_CODE_HASH()",returns=["bytes32"])[0]
    count=c.one(FACTORY,"allPairsLength()",returns=["uint256"])[0]
    rows=[decode(["address"],r)[0] for r in posts(c,[(FACTORY,"allPairs(uint256)",["uint256"],[i]) for i in range(count)])]
    methods=[("token0()",["address"]),("token1()",["address"]),("globalState()",["uint160","int24","uint16","uint8","uint16","bool"]),
             ("plugin()",["address"]),("liquidity()",["uint128"])]
    results=iter(posts(c,[(p,s,[],[]) for p in rows for s,_ in methods]))
    markets=[]
    for pool in rows:
        t0,t1,gs,plugin,liq=(decode(abi,next(results)) for _,abi in methods)
        markets.append(dict(pool=pool,tokens=[t0[0],t1[0]],state=list(map(str,gs)),plugin=plugin[0],liquidity=str(liq[0])))
    lookups=[]
    for m in markets:
        a,b=m["tokens"]; pool=m["pool"]
        lookups.extend([(FACTORY,"poolByPair(address,address)",["address","address"],[a,b]),
                        (FACTORY,"poolByPair(address,address)",["address","address"],[b,a]),
                        (REGISTRY,"pluginByPool(address)",["address"],[pool])])
    results=iter(posts(c,lookups))
    for m in markets:
        m["standard"]=all([decode(["address"],next(results))[0]==v for v in (m["pool"],m["pool"],m["plugin"])])
    state={EXEC:"0x"+RUNTIME.read_text().strip(),VIRTUAL:"0x"+VIRTUAL_FILE.read_text().strip()}
    if any(code(a) for a in [OWNER,EXEC,VIRTUAL]):raise ValueError("simulation address contains deployed code")
    deadline=int(head["timestamp"],16)+180
    evidence={}
    for m in markets:
        if not m["standard"] or not set(m["tokens"])<={ETH_WETH,USDT,USDC}:continue
        pool,pl=m["pool"],m["plugin"]
        url="https://eth.blockscout.com/api/v2/smart-contracts/"+pl
        res=requests.get(url,timeout=24);res.raise_for_status();record=res.json()
        live=code(pl);word=bytes(12)+bytes.fromhex(pool[2:])
        positions=[i for i in range(len(live)) if live.startswith(word,i)]
        if (not record.get("is_verified") or record.get("is_changed_bytecode") or record["name"]!="AlgebraBasePluginV3"
                or bytes.fromhex(record["deployed_bytecode"][2:])!=live or not positions or not all(live[i-1]==0x7f for i in positions)):
            raise ValueError("verified plugin runtime and immutable address differ")
        evidence[pool]=dict(source=url, runtime=digest(live), normalized=digest(live.replace(word,bytes(32))), offsets=positions)
        security=address(pl,"getSecurityRegistry()"); discounts=address(pl,"feeDiscountRegistry()")
        m.update(reflex=c.one(pl,"isReflexEnabled()",returns=["bool"])[0], security=security,discount_registry=discounts,
                 pool_status=c.one(security,"getPoolStatus(address)",["address"],[pool],["uint8"])[0],
                 plugin_fee=c.one(pl,"getCurrentFee()",returns=["uint16"])[0],wallet=[])
        for n,a in enumerate(m["tokens"]):
            b=m["tokens"][1-n]; funding=HOLDERS["ethereum"][a]
            for dx in ((10**14,10**15) if a==ETH_WETH else (10**6,10**7)):
                discount=c.one(discounts,"feeDiscounts(address,address)",["address","address"],[OWNER,pool],["uint16"])[0]
                r=simulate(e,pool,QUOTER,m["tokens"][0],a,b,dx,state,deadline,OWNER,True,ETH_WETH,funding)
                r["discount"]=discount;m["wallet"].append(r)
    if not evidence:raise ValueError("no on-chain registered stablecoin / wrapped ETH markets")
    return dict(selector=tag,block=e["block"],state_root=ROOT_HASH,endpoint=e["endpoint"],factory=FACTORY,
                quoter=QUOTER,router=ROUTER,plugin_factory=REGISTRY,pool_deployer=DEPLOYER,
                pair_init="0x"+init.hex(),contracts={a:digest(code(a)) for a in (FACTORY,REGISTRY,QUOTER,ROUTER,DEPLOYER)},
                plugin_sources=evidence, markets=markets)


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument("output",type=Path);args=parser.parse_args()
    snapshot=read(FOLDER/"ethereum.json");errors=[]
    for endpoint in ENDPOINTS:
        try:
            result=capture(dict(snapshot,endpoint=endpoint))
            args.output.write_text(json.dumps(result,indent=2)+"\n",encoding="utf8")
            break
        except (requests.RequestException,KeyError,ValueError,RuntimeError) as err:
            errors.append((endpoint,str(err)[:240]))
    else:raise RuntimeError(f"canonical archive failed: {errors}")
