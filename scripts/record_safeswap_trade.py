"""Оплата приказа своим кошельком через общедоступный SafeSwapTradeRouter (Ethereum, 2022).
Сумма ETH-комиссии независима от ERC20 списания. При чужом начальном остатке, который
мог бы участвовать в платеже, тест не запускает swap. Состояние воспроизводится только в eth_simulateV1.
"""
from eth_abi import decode
from record_base_slipstream_executor import OWNER, rpc, simulate_block
from record_bitswap_ethereum import balance, tx
from record_ethereum_v2_factories import WETH, digest, raw

FACTORY_SLOT="0xb2101b231486a8a17a16c101f8dde1145d21799358462f57035a227f25614da3"
MULTICALL="0xca11bde05977b3631167028862be2a173976ca11"


def roles(c,row,verified,slot,implementation_slot):
    factory=row["roles"]["factory"]["address"]
    router=row["roles"]["router"]["address"]
    public,=c.one(router,"routerTrade()",returns=["address"])
    if c.one(public,"swapRouter()",returns=["address"])!=(router,) or c.one(factory,"router()",returns=["address"])!=(router,):
        raise ValueError("unbound public trade path")
    if c.one(router,"whitelistAccess(address)",["address"],[public],["bool"])!=(True,):
        raise ValueError("public entry has no access")
    version,=c.one(router,"version()",returns=["uint256"])
    delegate,=c.one(router,"impls(uint256)",["uint256"],[version],["address"])
    public_shell,_=verified(c,public,False)
    access,_=verified(c,slot(c,public,implementation_slot),False)
    engine,_=verified(c,delegate,False)
    pair,=c.one(factory,"implementation()",returns=["address"])
    code=raw(rpc(c.endpoint,"eth_getCode",[pair,c.selector]))
    factory_code=raw(rpc(c.endpoint,"eth_getCode",[row["implementations"]["factory"]["address"],c.selector]))
    if (len(code)<1000 or factory_code.count(code)!=1 or access["name"]!="SafeSwapTradeRouter"
            or engine["name"]!="SafeswapRouterProxy2"):
        raise ValueError("unrecognized public swap implementation")
    for m in row["markets"]:
        if slot(c,m["pool"],FACTORY_SLOT)!=factory:raise ValueError("pair implementation resolver differs")
    return dict(trade=public_shell,implementation=access,router_version=version,router_delegate=engine,
                pair_implementation=pair,pair_hash=digest(code),pair_source_offset=factory_code.index(code))


def wallets(c,row,stamp):
    router=row["roles"]["router"]["address"]
    public=row["access"]["trade"]["address"]
    end=stamp+240
    env={OWNER:{"balance":hex(10**19)}}
    if rpc(c.endpoint,"eth_getCode",[OWNER,c.selector])!="0x":
        raise ValueError("test payer must be an unoccupied address")

    def run(calls):
        return simulate_block(c.endpoint,c.selector,env,calls)

    def value(result):
        if result["status"]!="0x1" or len(result["returnData"])!=66:raise ValueError("balance read failed")
        return int(result["returnData"],16)

    def ether(who):
        return tx(MULTICALL,"getEthBalance(address)",["address"],[who])

    def leg(prefix,a,b,size):
        quote=tx(public,"getFees(address[],uint256,address)",["address[]","uint256","address"],[[a,b],size,OWNER])
        fee_reads=run([*prefix,quote,ether(public)])
        if any(r["status"]!="0x1" for r in fee_reads):raise ValueError("funding or public fee getter reverted")
        fees=decode(["uint256"]*4,raw(fee_reads[-2]["returnData"]))
        if fees[2] or fees[3]:raise ValueError("token-specific fee requires its own measured delivery")
        fee=fees[1]
        if value(fee_reads[-1])!=0:raise ValueError("pre-existing fee collector ETH")
        approval=tx(a,"approve(address,uint256)",["address","uint256"],[router,size])
        assets=[balance(a,OWNER),balance(b,OWNER),ether(OWNER),balance(a,public),balance(b,public),ether(public)]
        def trial(minimum,expiry):
            command=tx(public,"swapExactTokensForTokensWithFeeAmount((uint256,uint256,address[],address,uint256))",
                       ["(uint256,uint256,address[],address,uint256)"],[(size,minimum,[a,b],OWNER,expiry)],fee)
            answers=run([*prefix,approval,*assets,command,*assets])
            pos=len(prefix)+1; n=len(assets)
            if any(v["status"]!="0x1" for v in answers[:pos]):raise ValueError("origin funding or approval reverted")
            before=list(map(value,answers[pos:pos+n])); after=list(map(value,answers[-n:])); winner=answers[pos+n]
            if winner["status"]=="0x1":
                if before[0]-after[0]!=size or before[2]-after[2]!=fee or after[3:]!=before[3:]:
                    raise ValueError("unexpected physical debit, fee or public inventory")
            elif before!=after:raise ValueError("rollback changed wallet inventory")
            return dict(status=winner["status"],input=a,output=b,spent=str(before[0]-after[0]),received=str(after[1]-before[1]),
                        fee=str(before[2]-after[2]),gas=winner["gasUsed"],approval_gas=answers[pos-1]["gasUsed"],
                        error=winner.get("error"),transaction=command,logs=winner.get("logs",[]),before=list(map(str,before)),after=list(map(str,after))),command
        loose,_=trial(1,end)
        if loose["status"]!="0x1":raise ValueError(str(loose["error"])[:155])
        received=int(loose["received"])
        if received<=0:raise ValueError("no payout to payer")
        real,command=trial(received,end)
        above,_=trial(received+1,end)
        expired,_=trial(received,1)
        if real["status"]!="0x1" or real["received"]!=str(received) or any(r["status"]!="0x0" for r in (above,expired)):
            raise ValueError("public swap does not enforce wallet minimum or deadline")
        return dict(exact=real,fees=list(map(str,fees)),above=above["error"],expired=expired["error"]),[*prefix,approval,command]

    for m in row["markets"]:
        if WETH not in m["tokens"]:continue
        other=next(t for t in m["tokens"] if t!=WETH)
        reserves=int(m["reserves"][m["tokens"].index(WETH)])
        sizes=sorted({max(1,min(10**15,reserves//1000)), max(1,min(10**16,reserves//16))})
        samples=m["paid_public"]=[]
        for amount in sizes:
            item={"size":str(amount)}
            samples.append(item)
            try:
                buy,prefix=leg([tx(WETH,"deposit()",value=amount)],WETH,other,amount)
                item["buy"]=buy
                sell,_=leg(prefix,other,WETH,int(buy["exact"]["received"]))
                item["sell"]=sell
            except ValueError as error:
                item["unavailable"]=str(error)[:220]
