"""Reconstruct every Supernova CL creation event, indexed pool, plugin and fee configuration at the accepted header."""
import argparse
import json
from pathlib import Path
from eth_abi import decode, encode
from eth_utils import keccak
from record_balancer_markets import Capture, posts
from record_ethereum_x7 import ENDPOINTS, ROOT_HASH
from record_hydrex_integral import FOLDER, read, rpc
from record_supernova_ethereum import FACTORY, QUOTER, REGISTRY, ROUTER, DEPLOYER, digest
from record_base_classic_directory import SLOT


def collect(endpoint):
    e=dict(read(FOLDER/"ethereum.json"), endpoint=endpoint)
    c=Capture(e); tag=e["selector"]
    h=rpc(e,"eth_getBlockByHash",[tag["blockHash"],False])
    if (int(rpc(e,"eth_chainId",[]),16),h["hash"],int(h["number"],16),h["stateRoot"])!=(1,tag["blockHash"],e["block"],ROOT_HASH):
        raise ValueError("directory header mismatch")
    count,=c.one(FACTORY,"allPairsLength()",returns=["uint256"])
    init,=c.one(FACTORY,"POOL_INIT_CODE_HASH()",returns=["bytes32"])
    rows=[decode(["address"],v)[0] for v in posts(c,[(FACTORY,"allPairs(uint256)",["uint256"],[i]) for i in range(count)])]
    logs=rpc(e,"eth_getLogs",[dict(address=FACTORY,fromBlock=hex(24390427),toBlock=hex(e["block"]),
                                    topics=[digest(b"CustomPool(address,address,address,address)")])])
    created={"0x"+v["data"][-40:]:v for v in logs if not v.get("removed",False)}
    if set(rows)!=set(created) or len(created)!=count or len(logs)!=count:
        raise ValueError("creation events and allPairs do not match uniquely")
    abi=["uint160","int24","uint16","uint8","uint16","bool"]
    methods=[("token0()",["address"]),("token1()",["address"]),("plugin()",["address"]),
             ("liquidity()",["uint128"]),("globalState()",abi)]
    replies=iter(posts(c,[(p,s,[],[]) for p in rows for s,_ in methods]))
    markets=[];lookups=[]
    for p in rows:
        a,b,plugin,liq,gs=(decode(types,next(replies)) for _,types in methods)
        a,b,plugin=a[0],b[0],plugin[0];log=created[p];role="0x"+log["topics"][1][-40:]
        salt=keccak(encode(["address"]*3,[role,a,b]))
        if "0x"+keccak(bytes([255])+bytes.fromhex(DEPLOYER[2:])+salt+init)[-20:].hex()!=p:
            raise ValueError("CREATE2 mismatch")
        markets.append(dict(pool=p,tokens=[a,b],fee=gs[2],flags=gs[3],community_fee=gs[4],plugin=plugin,
                            liquidity=str(liq[0]),custom=role,create2=True,created=dict(block=int(log["blockNumber"],16),
                            block_hash=log["blockHash"],tx=log["transactionHash"],index=int(log["logIndex"],16))))
        lookups.extend([(FACTORY,"customPoolByPair(address,address,address)",["address"]*3,[role,*t]) for t in ((a,b),(b,a))])
        lookups.extend([(REGISTRY,"pluginByPool(address)",["address"],[p]),(plugin,"feeConfig()",[],[])])
    results=iter(posts(c,lookups));code=lambda a:bytes.fromhex(rpc(e,"eth_getCode",[a,tag])[2:])
    for m in markets:
        if [decode(["address"],next(results))[0] for _ in range(3)]!=[m["pool"],m["pool"],m["plugin"]]:
            raise ValueError("custom registry or plugin differs")
        cfg=decode(["uint16","uint16","uint32","uint32","uint16","uint16","uint16"],next(results))
        live=code(m["plugin"]);word=bytes(12)+bytes.fromhex(m["pool"][2:])
        offsets=[i for i in range(len(live)) if live.startswith(word,i)]
        if not offsets or any(live[i-1]!=127 for i in offsets):raise ValueError("unrecognized immutable binding")
        m.update(registered=True,fee_config=cfg,plugin_hash=digest(live),template=digest(live.replace(word,bytes(32))),slots=len(offsets))
    security,=c.one(markets[0]["plugin"],"getSecurityRegistry()",returns=["address"])
    discount,=c.one(markets[0]["plugin"],"feeDiscountRegistry()",returns=["address"])
    roles={name:dict(address=a,hash=digest(code(a)),slot=rpc(e,"eth_getStorageAt",[a,SLOT,tag]))
           for name,a in dict(factory=FACTORY,quoter=QUOTER,router=ROUTER,plugin_factory=REGISTRY,deployer=DEPLOYER,
                              security=security,discount=discount).items()}
    return dict(selector=tag,state_root=ROOT_HASH,markets=markets,chain_id=1,block=e["block"],factory=FACTORY,
                deployer=DEPLOYER,quoter=QUOTER,registry=REGISTRY,init="0x"+init.hex(),roles=roles,
                custom_deployers=sorted({r["custom"] for r in markets}))


if __name__=="__main__":
    cmd=argparse.ArgumentParser(description=__doc__);cmd.add_argument("output",type=Path);a=cmd.parse_args()
    failures=[]
    for url in ENDPOINTS:
        try:
            result=collect(url);a.output.write_text(json.dumps(result,indent=2)+"\n",encoding="utf8");break
        except (ValueError,KeyError,OSError,RuntimeError) as err:failures.append(str(err)[:140])
    else:raise ValueError("could not reproduce the full registry: "+str(failures))
