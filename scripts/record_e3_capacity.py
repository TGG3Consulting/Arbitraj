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
            for fee in (500,3000,100,10000):
                params="(address,address,uint24,address,uint256,uint256,uint160)"
                buy=tx(UNI,"exactOutputSingle("+params+")",[params],[(WETH,src,fee,OWNER,cap+1,4*10**18,0)],4*10**18)
                warm=run([buy,read(src)])
                if ok(warm[0]) and int.from_bytes(result(warm[1]))>=cap+1:
                    funding=buy;break
        if funding is None:
            rows.append(dict(pool=pool,input=src,output=dst,capacity=cap,unfunded=True));continue
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
                calls=run([funding,allowance,read(src),read(dst),order,read(src),read(dst)])
                assert all(ok(calls[i]) for i in (0,1,2,3,5,6))
                a,b,after_a,after_b=(int.from_bytes(result(calls[i])) for i in (2,3,5,6))
                spent=a-after_a;received=after_b-b;swap=calls[4]
                if ok(swap):
                    assert spent>0 and received>=minimum
                else:assert spent==0 and received==0
                rows.append(dict(pool=pool,input=src,output=dst,mode=mode,amount=amount,capacity=cap,nominal=minimum,left=left,fee=fee,
                                 spent=spent,received=received,success=ok(swap),error=swap.get("error"),gas=int(swap["gasUsed"],16)))
    return rows

