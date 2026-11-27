"""Discover Theorem's default and custom Ethereum pools through its CREATE2 deployer nonce."""
import argparse
import json
from pathlib import Path

from eth_abi import decode, encode
from eth_utils import keccak
from fork_fee_rpc import Reader, pool_blocks
from record_balancer_markets import posts
from record_cypher_v4 import CUSTOM, POOL, ZERO, plugin_template
from record_llama_base_cl import CanonicalCalls, ROOT

FACTORY = "0xd265f57c36ac60d3f7931ec5c7396966f0c246a7"
DEPLOYER = "0xfe3becd788320465ab649015f34f7771220a88b2"
QUOTER = "0xaab91422ad832f2a1e05ea8ab70cafdf0da597a1"
ROUTER = "0xf494e400e924188d9be68d128fa4a49606e5f1d8"
BEACON = "0xa3f0ad74e5423aebfd80d3ef4346578335a9a72aeaee59ff6cb3582b35133d50"
DATA = ROOT / "tests/data/first_run_dex_2026-11-26"
WEB = "https://app.theorem.markets/assets/index-V4IkL7Qa.js"

def directory(reader):
    cap=CanonicalCalls(reader);tag=reader.tag
    snap=json.loads((DATA/"ethereum.json").read_bytes())
    no_code,nonce=reader.many([("eth_getCode",[DEPLOYER,hex(24924400)]),("eth_getTransactionCount",[DEPLOYER,tag])])
    assert no_code=="0x"
    total=int(nonce,16)
    changes=sorted(pool_blocks(reader,DEPLOYER,24924400,snap["block"],0,total).items())
    headers=reader.many(("eth_getBlockByNumber",[hex(h),False]) for h,_ in changes)
    events=reader.many(("eth_getLogs",[dict(address=FACTORY,blockHash=h["hash"],topics=[[POOL,CUSTOM]])]) for h in headers)
    pools=[];created=False
    for (height,amount),head,rows in zip(changes,headers,events,strict=True):
        assert height==int(head["number"],16)
        # Account starts at nonce 1 on CREATE. Verify the factory's deployer already exists here.
        if not created:
            assert reader.one("eth_getCode",[DEPLOYER,hex(height-1)])=="0x"
            assert reader.one("eth_getCode",[DEPLOYER,hex(height)])!="0x"
            created=True;amount-=1
        assert len(rows)==amount
        for log in rows:
            topics=log["topics"]
            assert log["blockHash"]==head["hash"] and not log.get("removed") and log["address"].lower()==FACTORY
            custom=ZERO if topics[0]==POOL else "0x"+topics[1][-40:]
            assert len(topics)==(3 if custom==ZERO else 4)
            tokens=["0x"+t[-40:] for t in topics[-2:]]
            assert tokens[0]<tokens[1]
            pools.append(dict(pool=decode(["address"],bytes.fromhex(log["data"][2:]))[0],tokens=tokens,
                              deployer=custom,height=height,registration=log))
    assert len({m["pool"] for m in pools})==len(pools)==total-1
    roles=dict(factory=FACTORY,physical=DEPLOYER,quoter=QUOTER,router=ROUTER)
    default,=cap.one(FACTORY,"defaultPluginFactory()",returns=["address"]);roles["plugin_factory"]=default
    bindings=posts(cap,[(r,k,[],[]) for r in (QUOTER,ROUTER) for k in ("factory()","poolDeployer()")])
    assert [decode(["address"],v)[0] for v in bindings]==[FACTORY,DEPLOYER]*2
    init,=cap.one(FACTORY,"POOL_INIT_CODE_HASH()",returns=["bytes32"])
    assert cap.one(FACTORY,"poolDeployer()",returns=["address"])==(DEPLOYER,)
    reads=[]
    for m in pools:
        a,b=m["tokens"];p=m["pool"];c=m["deployer"]
        for x,y in [(a,b),(b,a)]:
            if c==ZERO:reads.append((FACTORY,"poolByPair(address,address)",["address"]*2,[x,y]))
            else:reads.append((FACTORY,"customPoolByPair(address,address,address)",["address"]*3,[c,x,y]))
        salt=encode(["address"]*2,[a,b]) if c==ZERO else encode(["address"]*3,[c,a,b])
        assert p=="0x"+keccak(b"\xff"+bytes.fromhex(DEPLOYER[2:])+keccak(salt)+init)[-20:].hex()
        reads.extend((p,k,[],[]) for k in ("token0()","token1()","factory()","liquidity()","tickSpacing()","plugin()","globalState()"))
        reads.extend((t,"balanceOf(address)",["address"],[p]) for t in [a,b])
    values=iter(posts(cap,reads));plug=set()
    def next_val(typ):return decode([typ],next(values))[0]
    for m in pools:
        assert [next_val("address") for _ in range(5)]==[m["pool"],m["pool"],*m["tokens"],FACTORY]
        m.update(liquidity=str(next_val("uint128")),spacing=next_val("int24"),plugin=next_val("address"))
        price,tick,fee,flags,community,unlocked=decode(["uint160","int24","uint16","uint8","uint16","bool"],next(values))
        m.update(price=str(price),tick=tick,fee=fee,flags=flags,community=community,unlocked=unlocked,
                 balances=[str(next_val("uint256")),str(next_val("uint256"))])
        if m["plugin"]!=ZERO:plug.add(m["plugin"])
    def code(address):
        raw=bytes.fromhex(reader.one("eth_getCode",[address,tag])[2:]);assert raw
        return dict(address=address,runtime="0x"+keccak(raw).hex(),bytes=len(raw)),raw
    plugins={}
    for m in pools:
        addr=m["plugin"]
        if addr==ZERO:continue
        bind,raw=code(addr)
        templ,refs=plugin_template(raw,m["pool"])
        bind.update(template=templ,pool_operands=refs)
        vals=posts(cap,[(addr,k,[],[]) for k in ("getPool()","getSecurityRegistry()","getActiveModuleNames()")])
        assert decode(["address"],vals[0])==(m["pool"],)
        bind.update(registry=decode(["address"],vals[1])[0],modules=decode(["string[]"],vals[2])[0])
        slot=reader.one("eth_getStorageAt",[addr,BEACON,tag])
        if int(slot,16):
            beacon="0x"+slot[-40:]
            impl,=cap.one(beacon,"implementation()",returns=["address"])
            bind.update(beacon=code(beacon)[0],implementation=code(impl)[0])
        plugins[addr]=bind
    assert plug==set(plugins)
    return dict(chain_id=1,height=snap["block"],selector=tag,state_root=reader.header["stateRoot"],
                source=WEB,nonce=total,init="0x"+init.hex(),roles={k:code(a)[0] for k,a in roles.items()},markets=pools,plugins=plugins)

if __name__=="__main__":
    cli=argparse.ArgumentParser(description=__doc__);cli.add_argument("output",type=Path);args=cli.parse_args()
    reader=Reader("ethereum",json.loads((DATA/"ethereum.json").read_bytes()))
    args.output.write_text(json.dumps(directory(reader),separators=(",",":")),encoding="utf8")
