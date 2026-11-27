"""Ethereum HopeSwap и Archly. Реестр CREATE2, действующая ставка и оплата из средств владельца."""
import argparse
import json
from pathlib import Path

import requests
from eth_abi import decode
from eth_utils import keccak
from record_balancer_markets import Capture, posts
from record_base_slipstream_executor import STEP, rpc
from record_bitswap_ethereum import BLOCK, OWNER, WETH, EXEC, tx
from record_concave_ethereum import wallet_cases
from record_ethereum_v2_factories import digest, raw, registry
from record_ethereum_x7 import ENDPOINTS, ROOT_HASH, verified
from record_v2_chain_pools import swap

PROTOCOLS={
    "hopeswap-ethereum":("0x26f53fbadeeb777fb2a122dc703433d79241b64e","0x219bd2d1449f3813c01204ee455d11b41d5051e9"),
    "archly-ethereum":("0xe8e2b714c57937e0b29c6abeaf00b52388cab598","0xe8876189a80b2079d8c0a7867e46c50361d972c1"),
}

def capture(name,endpoint):
    arch=name=="archly-ethereum"
    factory,router=PROTOCOLS[name]
    tag={"blockHash":BLOCK,"requireCanonical":True}
    c=Capture(dict(endpoint=endpoint,selector=tag))
    head=rpc(endpoint,"eth_getBlockByHash",[BLOCK,False])
    if head["stateRoot"]!=ROOT_HASH or head["hash"]!=BLOCK or rpc(endpoint,"eth_chainId",[])!="0x1":raise ValueError("иной блок или сеть")
    roles={}
    def role(key,address,contract):
        source,record,code=verified(c,address,contract)
        roles[key]=dict(address=address,runtime=digest(code),bytes=len(code),source=source)
        return record,code
    _,fcode=role("factory",factory,"PairFactory" if arch else "UniswapV2Factory")
    _,router_code=role("router",router,"Router" if arch else "UniswapV2Router02")
    if c.one(router,"factory()",returns=["address"])!=(factory,) or c.one(router,"weth()" if arch else "WETH()",returns=["address"])!=(WETH,):
        raise ValueError("иной роутер")
    count,=c.one(factory,"allPairsLength()",returns=["uint256"])
    pairs=[decode(["address"],v)[0] for v in posts(c,[(factory,"allPairs(uint256)",["uint256"],[i]) for i in range(count)])]
    nonce=int(rpc(endpoint,"eth_getTransactionCount",[factory,tag]),16)
    if len(set(pairs))!=count or count+1!=nonce:raise ValueError("реестр или nonce расходятся")
    if arch:
        h,=c.one(factory,"pairCodeHash()",returns=["bytes32"])
        init="0x"+h.hex()
        markets=[]
        for pair in pairs:
            a,b,stable,res=posts(c,[(pair,s,[],[]) for s in ["token0()","token1()","stable()","getReserves()"]])
            a,b=[decode(["address"],x)[0] for x in (a,b)];stable,=decode(["bool"],stable)
            salt=keccak(raw(a)+raw(b)+bytes([int(stable)]))
            want="0x"+keccak(b"\xff"+raw(factory)+salt+h)[-20:].hex()
            if a>=b or want!=pair or c.one(factory,"isPair(address)",["address"],[pair],["bool"])!=(True,):raise ValueError("чужой пул Archly")
            for tokens in [(a,b),(b,a)]:
                if c.one(factory,"getPair(address,address,bool)",["address","address","bool"],[*tokens,stable],["address"])!=(pair,):raise ValueError("иная регистрация")
            fee,=c.one(factory,"getRealFee(address)",["address"],[pair],["uint256"])
            override,=c.one(factory,"feesOverrides(address)",["address"],[pair],["uint256"])
            base,=c.one(factory,"stableFee()" if stable else "volatileFee()",returns=["uint256"])
            if fee!=(override or base):raise ValueError("не подтверждена комиссия пары")
            markets.append(dict(pool=pair,tokens=[a,b],reserves=list(map(str,decode(["uint256"]*3,res)[:2])),
                                stable=stable,fee_bps=fee,override=override,base_fee=base))
    else:
        rec,_=role("pair",pairs[0],"UniswapV2Pair")
        creation=raw(rec["creationBytecode"]["onchainBytecode"]);init=digest(creation)
        if fcode.count(creation)!=1:raise ValueError("байткод создания не найден в этой фабрике")
        markets=registry(c,factory,init)
        for m in markets:
            denominator,=c.one(m["pool"],"feeRateDenominator()",returns=["uint32"])
            retained,=c.one(m["pool"],"getFeeRateNumerator()",returns=["uint32"])
            if denominator!=10000 or not 9700<=retained<=10000:raise ValueError("недопустимая ставка")
            m.update(fee_bps=denominator-retained,retained=retained,denominator=denominator)
    for m in markets:
        code=raw(rpc(endpoint,"eth_getCode",[m["pool"],tag]))
        m.update(runtime=digest(code),bytes=len(code),balances=[str(c.one(t,"balanceOf(address)",["address"],[m["pool"]],["uint256"])[0]) for t in m["tokens"]])
        if not arch and m["runtime"]!=roles["pair"]["runtime"]:raise ValueError("иная реализация пары")
    def market(a,b):
        rows=[m for m in markets if sorted((a,b))==m["tokens"]]
        if len(rows)!=1:raise ValueError("неоднозначная пара в маршруте подготовки")
        return rows[0]
    path_type="(address,address,bool)[]" if arch else "address[]"
    method="swapExactTokensForTokensSupportingFeeOnTransferTokens(uint256,uint256,"+path_type+",address,uint256)"
    if keccak(text=method)[:4] not in router_code:raise ValueError("платёжный селектор отсутствует")
    def direct(a,b,n,limit,end):
        route=[(a,b,market(a,b)["stable"])] if arch else [a,b]
        return tx(router,method,["uint256","uint256",path_type,"address","uint256"],[n,limit,route,OWNER,end])
    def executor(m,a,b,n,limit,end):
        if not arch:return swap(m["pool"],a,b,n,limit,end,m["fee_bps"]*100)
        return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
            ["address","uint256","address","uint256","uint256",STEP+"[]"],[a,n,b,limit,end,[(3,m["pool"],a,b,10000,b"")]])
    fills,missing=wallet_cases(endpoint,tag,int(head["timestamp"],16)+200,markets,router,direct,executor)
    return dict(protocol=name,selector=tag,height=int(head["number"],16),state_root=ROOT_HASH,roles=roles,nonce=nonce,pair_init=init,
                markets=markets,owner=OWNER,cases=fills,unavailable=missing)

if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("protocol",choices=PROTOCOLS);parser.add_argument("output",type=Path);args=parser.parse_args()
    failures=[]
    for endpoint in ENDPOINTS:
        try:
            d=capture(args.protocol,endpoint);args.output.write_text(json.dumps(d,separators=(",",":")));break
        except (requests.RequestException,ValueError,KeyError,RuntimeError) as err:failures.append((endpoint,str(err)[:500]))
    else:raise ValueError(failures)
