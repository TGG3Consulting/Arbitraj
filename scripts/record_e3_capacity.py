"""Фактический расход у границы полного исполнения E3, включая полный возврат при отказе."""
from eth_abi import decode
from record_e3_base import EXEC,OWNER,PATH,STEP,USDC,WETH,UNI,tx,ok,result

def examine(r,pool,tokens,step,router,stamp,run):
    rows=[]
    for index,src in enumerate(tokens):
        dst=tokens[1-index]
        def quote(a):return tx(pool,"getSwapOut(uint128,bool)",["uint128","bool"],[a,index==0])
        limit=10**28
        leftover,output,_=decode(["uint128"]*3,result(run([quote(limit)])[0]))
        cap=limit-leftover
        if cap==0 or output==0:continue
        read=lambda t:tx(t,"balanceOf(address)",["address"],[OWNER])
        funding=None
        if src==WETH:
            funding=tx(WETH,"deposit()",[],[],cap+1)
        else:
            budget=12*10**18
            for fee in (500,3000,100,10000):
                params="(address,address,uint24,address,uint256,uint256,uint160)"
                buy=tx(UNI,"exactOutputSingle("+params+")",[params],[(WETH,src,fee,OWNER,cap+1,budget,0)],budget)
                warm=run([buy,read(src)])
                if ok(warm[0]) and int.from_bytes(result(warm[1]))>=cap+1:
                    funding=buy;break
            if funding is None and src!=USDC:
                # exactOutput перечисляет токены в обратном порядке. Посредник USDC приобретается
                # средствами этого же кошелька, состояние проверяемого пула E3 остаётся исходным.
                for first_fee,second_fee in ((500,100),(500,500),(3000,100),(3000,500),(500,3000)):
                    path=bytes.fromhex(src[2:])+second_fee.to_bytes(3,"big")+bytes.fromhex(USDC[2:])+first_fee.to_bytes(3,"big")+bytes.fromhex(WETH[2:])
                    params="(bytes,address,uint256,uint256)"
                    buy=tx(UNI,"exactOutput("+params+")",[params],[(path,OWNER,cap+1,budget)],budget)
                    warm=run([buy,read(src)])
                    if ok(warm[0]) and int.from_bytes(result(warm[1]))>=cap+1:
                        funding=buy;break
            if funding is None and src!=USDC:
                from record_deliswap_base import AERO, FACTORY, ROUTES
                for stable in (True,False):
                    routes=[(WETH,USDC,False,FACTORY),(USDC,src,stable,FACTORY)]
                    buy=tx(AERO,"swapExactETHForTokens(uint256,"+ROUTES+",address,uint256)",["uint256",ROUTES,"address","uint256"],
                           [cap+1,routes,OWNER,stamp+90],budget)
                    warm=run([buy,read(src)])
                    if ok(warm[0]) and int.from_bytes(result(warm[1]))>=cap+1:
                        funding=buy;break
        if funding is None and src!=WETH:
            # Обычный сверхобеспеченный заём Aave V3 Base с собственным WETH.
            # https://github.com/bgd-labs/aave-address-book/blob/main/src/AaveV3Base.sol
            lending="0xa238dd80c259a72e81d7e4664a9801593f98d1c5"
            collateral=12*10**18
            deposit=tx(WETH,"deposit()",[],[],collateral)
            approve=tx(WETH,"approve(address,uint256)",["address","uint256"],[lending,collateral])
            supply=tx(lending,"supply(address,uint256,address,uint16)",["address","uint256","address","uint16"],[WETH,collateral,OWNER,0])
            borrow=tx(lending,"borrow(address,uint256,uint256,uint16,address)",["address","uint256","uint256","uint16","address"],[src,cap+1,2,0,OWNER])
            funding=[deposit,approve,supply,borrow]
            warm=run(funding+[read(src)])
            if not all(map(ok,warm)) or int.from_bytes(result(warm[-1]))<cap+1:
                funding=None
        if funding is None:
            rows.append(dict(pool=pool,input=src,output=dst,capacity=cap,unfunded=True));continue
        prefix=funding if isinstance(funding,list) else [funding]
        for mode,spender in [('official',router),('v5',EXEC)]:
            for amount in sorted({v for v in (cap-1,cap,cap+1) if v>0}):
                left,minimum,fee=decode(["uint128"]*3,result(run([quote(amount)])[0]))
                allowance=tx(src,"approve(address,uint256)",["address","uint256"],[spender,amount])
                if mode=="official":
                    types=["uint256","uint256",PATH,"address","uint256"]
                    order=tx(router,"swapExactTokensForTokensSupportingFeeOnTransferTokens("+",".join(types)+")",types,
                             [amount,minimum,([step],[2],[src,dst]),OWNER,stamp+90])
                else:
                    types=["address","uint256","address","uint256","uint256",STEP+"[]"]
                    order=tx(EXEC,"execute("+",".join(types)+")",types,[src,amount,dst,minimum,stamp+90,[(18,pool,src,dst,10000,b"")]])
                calls=run(prefix+[allowance,read(src),read(dst),order,read(src),read(dst)])
                assert all(map(ok,calls[:len(prefix)]))
                calls=calls[len(prefix):]
                assert all(ok(calls[i]) for i in (0,1,2,4,5))
                a,b,after_a,after_b=(int.from_bytes(result(calls[i])) for i in (1,2,4,5))
                spent=a-after_a;received=after_b-b;swap=calls[3]
                if ok(swap):
                    assert spent>0 and received>=minimum
                else:assert spent==0 and received==0
                rows.append(dict(pool=pool,input=src,output=dst,mode=mode,amount=amount,capacity=cap,nominal=minimum,left=left,fee=fee,
                                 spent=spent,received=received,success=ok(swap),error=swap.get("error"),gas=int(swap["gasUsed"],16)))
    return rows

