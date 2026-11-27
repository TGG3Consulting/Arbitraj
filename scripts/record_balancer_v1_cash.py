"""Original BPool exact-in/out cash, one-atom rejection and payment in an isolated replay."""
from record_base_slipstream_executor import OWNER, simulate_block
from record_bitswap_ethereum import balance, tx
from record_ethereum_v2_factories import FUNDING, WETH

UINT = "uint256"
MAX_PRICE = 2**256 - 1


def payments(c,rpc,pool,market,stable,timestamp):
    cases = []
    for source,target in ((WETH,stable),(stable,WETH)):
        src,dst = (market["assets"][t] for t in (source,target))
        for exact in (True,False):
            for divisor in (10**6,10**5):
                quantity = int((src if exact else dst)["balance"]) // divisor
                if not quantity:
                    continue
                inputs = [int(src["balance"]),int(src["weight"]),int(dst["balance"]),int(dst["weight"]),
                          quantity,int(market["swap_fee"])]
                func = "calcOutGivenIn" if exact else "calcInGivenOut"
                q, = c.one(pool,func+"(uint256,uint256,uint256,uint256,uint256,uint256)",[UINT]*6,inputs,[UINT])
                if not q:
                    continue
                spent,received = (quantity,q) if exact else (q,quantity)
                def trade(limit):
                    args = [source,quantity,target,limit,MAX_PRICE] if exact else [source,limit,target,quantity,MAX_PRICE]
                    name = "swapExactAmountIn" if exact else "swapExactAmountOut"
                    return tx(pool,name+"(address,uint256,address,uint256,uint256)",["address",UINT,"address",UINT,UINT],args)
                if source == WETH:
                    prep = [tx(WETH,"deposit()",value=spent)]
                else:
                    prep = [tx(FUNDING,"swapETHForExactTokens(uint256,address[],address,uint256)",
                               [UINT,"address[]","address",UINT],[spent,[WETH,source],OWNER,timestamp+600],value=10**18)]
                prep.append(tx(source,"approve(address,uint256)",["address",UINT],[pool,spent]))
                cash = [balance(source,OWNER),balance(target,OWNER)]
                limit = received if exact else spent
                calls = prep + cash + [trade(limit+(1 if exact else -1)),trade(limit)] + cash
                overrides = {OWNER:{"balance":hex(max(100*10**18,spent if source == WETH else 0))}}
                frame = simulate_block(c.endpoint,c.selector,overrides,calls,rpc)
                row = dict(pool=pool,source=source,target=target,exact_input=exact,amount=str(quantity),price=str(q))
                if any(v["status"] != "0x1" for v in frame[:len(prep)]):
                    row["external_funding"] = "unavailable"
                else:
                    a,b,strict,paid,x,y = frame[len(prep):]
                    if strict["status"] != "0x0" or paid["status"] != "0x1" or any(v["status"]!="0x1" for v in (a,b,x,y)):
                        raise ValueError(("limits or wallet",row,strict.get("error"),paid.get("error")))
                    actual = int(a["returnData"],16)-int(x["returnData"],16), int(y["returnData"],16)-int(b["returnData"],16)
                    if actual != (spent,received):
                        raise ValueError(("nonstandard balance",row,actual))
                    row.update(spent=str(spent),received=str(received),gas=int(paid["gasUsed"],16),logs=paid["logs"])
                cases.append(row)
    return cases
