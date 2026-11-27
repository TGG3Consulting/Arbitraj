"""Compare on-chain preview and physical recipient amounts after funding an independent wallet."""
from eth_abi import decode

from record_base_slipstream_executor import OWNER, simulate_block
from record_bitswap_ethereum import balance, tx
from record_ethereum_v2_factories import FUNDING, STABLES, WETH

UINT="uint256"
USD_POOL="0x4dece678ceceb27446b35c672dc7d61f30bad69e"


def why(result):
    error=result.get("error") or {}
    data=error.get("data") or result.get("returnData","")
    if isinstance(data,str) and data.startswith("0x08c379a0"):
        try:
            return decode(["string"],bytes.fromhex(data[10:]))[0][:100]
        except ValueError:
            pass
    return str(error.get("message","reverted"))[:120]


def measure(c,rpc,venue,entry,market,stable,timestamp):
    def simulation(calls):
        return simulate_block(c.endpoint,c.selector,{OWNER:{"balance":hex(10**18)}},calls,rpc)
    def approve(asset,spender,quantity):
        return tx(asset,"approve(address,uint256)",["address",UINT],[spender,quantity])
    def swap(pool,i,j,amount,minimum):
        call=tx(pool,"exchange(uint256,uint256,uint256,uint256,address)",
                [UINT,UINT,UINT,UINT,"address"],[i,j,amount,minimum,OWNER])
        call["gas"]=hex(6_000_000)
        return call
    usdc=STABLES[0]
    deposit=20*10**6
    prefix=[tx(FUNDING,"swapETHForExactTokens(uint256,address[],address,uint256)",
               [UINT,"address[]","address",UINT],[deposit,[WETH,usdc],OWNER,timestamp+600],value=10**17),
            approve(usdc,USD_POOL,deposit),
            tx(USD_POOL,"exchange(int128,int128,uint256,uint256)",["int128","int128",UINT,UINT],[0,1,deposit,1])]
    rows=[]
    for i in (0,1):
        j=1-i
        src,dst=entry["coins"][i],entry["coins"][j]
        prep=list(prefix)
        funding="USDC/crvUSD"
        if i==1 and src==WETH:
            prep=[tx(WETH,"deposit()",value=10**16)]
            funding="WETH.deposit"
        elif i==1:
            qty=10*10**18
            prep.append(approve(stable,market["cryptopool"],qty))
            if entry["kind"]=="amm":
                prep.append(tx(market["cryptopool"],"add_liquidity(uint256[2],uint256)",["uint256[2]",UINT],[[qty,0],1]))
                funding="mint collateral LP in underlying cryptopool"
            else:
                prep.append(swap(market["cryptopool"],0,1,qty,1))
                funding="asset purchase in underlying cryptopool"
        funding_frame=simulation(prep+[balance(src,OWNER)])
        failed=next((k for k,r in enumerate(funding_frame) if r["status"]!="0x1"),None)
        if failed is not None:
            rows.append(dict(side=i,stage="funding",funding=funding,step=failed,reason=why(funding_frame[failed])))
            continue
        available=int(funding_frame[-1]["returnData"],16)
        for divisor in (10,2):
            amount=available//divisor
            if not amount:
                rows.append(dict(side=i,stage="empty funding",funding=funding))
                break
            row=dict(side=i,source=src,target=dst,amount=str(amount),funding=funding)
            quote=tx(venue,"get_dy(uint256,uint256,uint256)",[UINT]*3,[i,j,amount])
            quote["gas"]=hex(6_000_000)
            # A state-changing preparation (for instance minting LP) is repeated identically before every observation.
            cash=[balance(src,OWNER),balance(dst,OWNER)]
            before=prep+[approve(src,venue,amount)]+cash+[quote]
            trial=simulation(before+[swap(venue,i,j,amount,1)]+cash)
            prefix_error=next((r for r in trial[:len(before)-1] if r["status"]!="0x1"),None)
            if prefix_error:
                rows.append(dict(row,stage="approval or cash",reason=why(prefix_error)))
                continue
            estimates=trial[len(before)-1]
            if estimates["status"]=="0x1":
                row["preview"]=str(int(estimates["returnData"],16))
            else:
                row["preview_error"]=why(estimates)
            paid=trial[len(before)]
            if paid["status"]!="0x1":
                rows.append(dict(row,stage="pool rejected",reason=why(paid)))
                continue
            left,right=trial[len(before)-3:len(before)-1]
            x,y=trial[-2:]
            spent=int(left["returnData"],16)-int(x["returnData"],16)
            received=int(y["returnData"],16)-int(right["returnData"],16)
            if spent!=amount or received<=0:
                raise ValueError(("unaccounted original payment",venue,i,spent,received))
            strict=simulation(before+[swap(venue,i,j,amount,received+1),swap(venue,i,j,amount,received)]+cash)
            deny,ok,after_in,after_out=strict[-4:]
            if deny["status"]!="0x0" or ok["status"]!="0x1" or x["returnData"]!=after_in["returnData"] or y["returnData"]!=after_out["returnData"]:
                raise ValueError(("one-atom bound",venue,i,deny.get("error"),ok.get("error")))
            rows.append(dict(row,stage="paid",spent=str(spent),received=str(received),gas=int(ok["gasUsed"],16),logs=ok["logs"]))
    return rows
