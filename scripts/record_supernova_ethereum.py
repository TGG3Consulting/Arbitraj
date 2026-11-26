"""Read the complete Supernova CL directory and compare actual V5 wallet settlement against the Algebra quoter."""
import argparse
import json
from pathlib import Path

from eth_abi import decode, encode
from eth_utils import keccak
from record_base_slipstream_executor import OWNER
from record_hydrex_integral import EXEC, FOLDER, RUNTIME, VIRTUAL, VIRTUAL_FILE, map_ticks, read, simulate
from record_solidly_v3 import ETH_WETH, HOLDERS
from record_ethereum_x7 import ROOT_HASH
from community_v3_rpc import ENDPOINTS
from derpdex_archive_rpc import Readers

FACTORY="0x44b7fbd4d87149efa5347c451e74b9fd18e89c55"
QUOTER="0xf9439cd803dcb11fa574bcc8421207f89b529e41"
REGISTRY="0xdbfd67d12cadb8925c1417ff3638693f2bf99b97"
DEPLOYER="0x45bc0f9855a626743d57d37d989f3b9462deba45"
ROUTER="0x72d63a5b080e1b89cc93f9b9f50cbfa5e291c8ac"
USDT="0xdac17f958d2ee523a2206206994597c13d831ec7"
USDC="0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"
LINK="0x514910771af9ca656af840dff83e8264ecf986ca"
XAUT="0x68749665ff8d2d112fa859aa293f07a622782f38"
# FeeDiscount(user,pool,1000) from the verified registry on Ethereum block 24546941.
DISCOUNT_EOA="0x62500df60073e22c5ff833ae855f7922bb4c0a88"
DISCOUNT_TX="0x86cdb96c35a99c2c8b981148b3fe30775fa10fd94a6696a2bf36c2d6c8be63b0"
SIZES={USDC:(10**6,10**7),USDT:(10**6,10**7),ETH_WETH:(10**14,10**15),
       LINK:(10**16,10**17),XAUT:(100,1000)}
WALLETS={**HOLDERS["ethereum"],LINK:HOLDERS["ethereum"][ETH_WETH],XAUT:HOLDERS["ethereum"][ETH_WETH]}

def chosen(tokens):
    pair=set(tokens)
    return pair<={ETH_WETH,USDT,USDC} or pair in ({ETH_WETH,LINK},{ETH_WETH,XAUT})



def digest(b):
    return "0x"+keccak(b).hex()


class Canonical:
    def __init__(self, reader, tag):
        self.reader,self.tag=reader,tag
    def one(self, address, method, args=(), values=(), returns=()):
        data="0x"+(keccak(text=method)[:4]+encode(args,values)).hex()
        raw=self.reader.read(None,"eth_call",[{"to":address,"data":data},self.tag])
        return decode(returns,bytes.fromhex(raw[2:]))


def capture(e):
    tag=e["selector"]
    directory=read(FOLDER/"supernova_ethereum_directory.json")
    if (directory["selector"], directory["state_root"], directory["block"])!=(tag,ROOT_HASH,e["block"]):
        raise ValueError("wallet replay and on-chain registry refer to different canonical states")
    marker={VIRTUAL:{"code":"0x602a60005260206000f3"}}
    reader=Readers(tag,e["block"],ROOT_HASH,FACTORY,marker,{"to":VIRTUAL,"data":"0x"},
                   "0x"+format(42,"064x"),urls=ENDPOINTS,chain=1,word_reads=False)
    rpc=reader.read
    c=Canonical(reader,tag)
    head=rpc(e,"eth_getBlockByHash",[tag["blockHash"],False])
    def address(a,s):return c.one(a,s,returns=["address"])[0]
    code=lambda a: bytes.fromhex(rpc(e,"eth_getCode",[a,tag])[2:])
    init=c.one(FACTORY,"POOL_INIT_CODE_HASH()",returns=["bytes32"])[0]
    count=c.one(FACTORY,"allPairsLength()",returns=["uint256"])[0]
    if count!=len(directory["markets"]) or "0x"+init.hex()!=directory["init"]:
        raise ValueError("factory index length or init code differs")
    if any((address(a,"factory()"),address(a,"poolDeployer()"))!=(FACTORY,DEPLOYER) for a in (QUOTER,ROUTER)):
        raise ValueError("unexpected periphery")
    markets=[]
    for i,m in enumerate(directory["markets"]):
        a,b=m["tokens"]
        if not chosen(m["tokens"]):continue
        p=m["pool"];custom=m["custom"]
        salt=keccak(encode(["address"]*3,[custom,a,b]))
        derived="0x"+keccak(bytes([255])+bytes.fromhex(DEPLOYER[2:])+salt+init)[-20:].hex()
        found=c.one(FACTORY,"allPairs(uint256)",["uint256"],[i],["address"])[0]
        registered=[c.one(FACTORY,"customPoolByPair(address,address,address)",["address"]*3,[custom,*t],["address"])[0] for t in ((a,b),(b,a))]
        plugin=address(p,"plugin()")
        if (found!=p or derived!=p or registered!=[p,p] or (address(p,"token0()"),address(p,"token1()"))!=(a,b)
                or c.one(REGISTRY,"pluginByPool(address)",["address"],[p],["address"])[0]!=plugin or m["plugin"]!=plugin):
            raise ValueError("wrong market for the requested deployment")
        gs=c.one(p,"globalState()",returns=["uint160","int24","uint16","uint8","uint16","bool"])
        markets.append(dict(m,state=list(map(str,gs)),creation={"deployer":custom}))
    state={EXEC:"0x"+RUNTIME.read_text().strip(),VIRTUAL:"0x"+VIRTUAL_FILE.read_text().strip()}
    if any(code(a) for a in [OWNER,EXEC,VIRTUAL]):raise ValueError("simulation address contains deployed code")
    deadline=int(head["timestamp"],16)+180
    evidence={}
    for m in markets:
        if not m["registered"] or not m["create2"]:continue
        pool,pl=m["pool"],m["plugin"]
        url="https://eth.blockscout.com/api/v2/smart-contracts/"+pl
        live=code(pl);word=bytes(12)+bytes.fromhex(pool[2:])
        positions=[i for i in range(len(live)) if live.startswith(word,i)]
        if (digest(live)!=m["plugin_hash"] or digest(live.replace(word,bytes(32)))!=m["template"]
                or not positions or not all(live[i-1]==0x7f for i in positions)):
            raise ValueError("verified plugin runtime and immutable address differ")
        evidence[pool]=dict(source=url, runtime=digest(live), normalized=digest(live.replace(word,bytes(32))), offsets=positions)
        security=address(pl,"getSecurityRegistry()"); discounts=address(pl,"feeDiscountRegistry()")
        m.update(reflex=c.one(pl,"isReflexEnabled()",returns=["bool"])[0], security=security,discount_registry=discounts,
                 pool_status=c.one(security,"getPoolStatus(address)",["address"],[pool],["uint8"])[0],
                 plugin_fee=c.one(pl,"getCurrentFee()",returns=["uint16"])[0],wallet=[])
        reference=c.one(discounts,"feeDiscounts(address,address)",["address","address"],[pool,pool],["uint16"])[0]
        discount=c.one(discounts,"feeDiscounts(address,address)",["address","address"],[OWNER,pool],["uint16"])[0]
        signature="beforeSwap(address,address,bool,int256,uint160,bool,bytes)"
        payload=keccak(text=signature)[:4]+encode(["address","address","bool","int256","uint160","bool","bytes"],[EXEC,OWNER,True,0,0,False,b""])
        raw=rpc(e,"eth_call",[{"from":pool,"to":pl,"data":"0x"+payload.hex()},tag])
        sel,fee,add=decode(["bytes4","uint24","uint24"],bytes.fromhex(raw[2:]))
        if sel!=keccak(text=signature)[:4] or add or m["pool_status"]!=0:
            raise ValueError("beforeSwap rejected the canonical pool")
        m.update(discount=discount,pool_discount=reference,hook_fee=fee,roles={
            security:digest(code(security)),discounts:digest(code(discounts))})
        for n,a in enumerate(m["tokens"]):
            b=m["tokens"][1-n]; funding=WALLETS[a]
            if c.one(a,"balanceOf(address)",["address"],[funding],["uint256"])[0]<max(SIZES[a]):
                raise ValueError("funding account cannot provide this size")
            for dx in SIZES[a]:
                r=simulate(e,pool,QUOTER,m["tokens"][0],a,b,dx,state,deadline,OWNER,True,ETH_WETH,funding,m["custom"],read_rpc=rpc)
                r["discount"]=discount;m["wallet"].append(r)
        m["discounted_wallet"]=[]
        if set(m["tokens"])=={ETH_WETH,USDT}:
            verified=c.one(discounts,"feeDiscounts(address,address)",["address","address"],[DISCOUNT_EOA,pool],["uint16"])[0]
            if verified!=1000 or code(DISCOUNT_EOA):
                raise ValueError("historically discounted externally owned wallet differs")
            for n,a in enumerate(m["tokens"]):
                b=m["tokens"][1-n]
                trial=simulate(e,pool,QUOTER,m["tokens"][0],a,b,SIZES[a][0],state,deadline,DISCOUNT_EOA,
                               True,ETH_WETH,WALLETS[a],m["custom"],read_rpc=rpc)
                if trial["override_fee"]!=0 or trial["plugin_fee"]!=0:
                    raise ValueError("discounted payer was charged an override")
                trial.update(discount=verified,discount_tx=DISCOUNT_TX);m["discounted_wallet"].append(trial)
        m["tick_window"]=map_ticks(c,pool,[int(m["state"][1])]+[r["tick_after"] for r in m["wallet"]+m["discounted_wallet"]])
    if {m["custom"] for m in markets if len(m["wallet"])==4}!=set(directory["custom_deployers"]):
        raise ValueError("incomplete V5 settlement across the four deployers")
    return dict(selector=tag,block=e["block"],state_root=ROOT_HASH,verified_endpoints=reader.urls,completed_reads=reader.counts,factory=FACTORY,
                quoter=QUOTER,router=ROUTER,plugin_factory=REGISTRY,pool_deployer=DEPLOYER,
                pair_init="0x"+init.hex(),contracts={a:digest(code(a)) for a in (FACTORY,REGISTRY,QUOTER,ROUTER,DEPLOYER)},
                plugin_sources=evidence, markets=markets)


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument("output",type=Path);args=parser.parse_args()
    result=capture(read(FOLDER/"ethereum.json"))
    args.output.write_text(json.dumps(result,indent=2)+"\n",encoding="utf8")
