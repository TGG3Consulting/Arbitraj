"""Measure fee-supporting Router and V5 on the same child state using only the payer's funds."""
from eth_utils import keccak
from record_base_slipstream_executor import EXEC, OWNER, RUNTIME, STEP, simulate_block
from record_bitswap_ethereum import tx, balance
from record_concave_ethereum import approval
from record_llama_base_cl import WETH, fund_quote_asset
from record_virtus_cl_base import USDC

ROUTE = "(address,address,bool,address)[]"
SWAP = "0x" + keccak(text="Swap(address,address,uint256,uint256,uint256,uint256)").hex()


def measure(reader, cap, book):
    assert reader.many(("eth_getCode",[a,cap.selector]) for a in [EXEC,OWNER])==["0x","0x"]
    word=lambda v:"0x"+format(v,"064x")
    overrides={OWNER:{"balance":hex(10**18)}, EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
        "stateDiff":{word(0):word(int(OWNER,16)),word(1):word(int(WETH,16))}}}
    def run(calls):
        def read(_url, method, params):
            assert method=="eth_simulateV1" and params[-1]==cap.selector
            return reader.one(method,params)
        return simulate_block(cap.endpoint,cap.selector,overrides,calls,read_rpc=read)
    deadline=int(reader.header["timestamp"],16)+120
    factory=book["roles"]["factory"]["address"];router=book["roles"]["router"]["address"]
    cases=[];refused=[];funding=[]
    def order(form,m,a,b,qty,minimum,due):
        if form=="v5":
            return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                      ["address","uint256","address","uint256","uint256",STEP+"[]"],
                      [a,qty,b,minimum,due,[(3,m["pool"],a,b,10000,b"")]])
        return tx(router,"swapExactTokensForTokensSupportingFeeOnTransferTokens(uint256,uint256,"+ROUTE+",address,uint256)",
                  ["uint256","uint256",ROUTE,"address","uint256"],[qty,minimum,[(a,b,m["stable"],factory)],OWNER,due])
    def prepare(a,seed):
        if a==WETH:return [tx(WETH,"deposit()",value=seed)],seed
        if a==USDC:
            calls,count,row=fund_quote_asset(cap,run,seed,deadline,a)
            funding.append(row);return calls,count
        m=next(m for m in book["markets"] if set(m["tokens"])=={a,WETH})
        fund=[tx(WETH,"deposit()",value=seed),approval(WETH,router,seed)]
        swap=order("router",m,WETH,a,seed,1,deadline)
        reads=[balance(t,OWNER) for t in [WETH,a]]
        rows=run(fund+reads+[swap]+reads)
        if any(r["status"]!="0x1" for r in rows): return [],0
        old,now=raw(rows[2:4]),raw(rows[-2:])
        q=now[1]-old[1]
        assert old[0]-now[0]==seed and q>0
        funding.append(dict(source=m["pool"],spent=str(seed),received=str(q),gas=int(rows[4]["gasUsed"],16)))
        return fund+[swap],q
    raw=lambda rows:[int(r["returnData"],16) for r in rows]
    available={WETH, USDC} | {t for m in book["markets"] if WETH in m["tokens"] for t in m["tokens"]}
    for m in book["markets"]:
        tokens=m["tokens"]
        base=next((a for a in (WETH, USDC,*tokens) if a in tokens and a in available),None)
        if not base:
            refused.append(dict(pool=m["pool"],reason="no funded ingress"));continue
        other=next(a for a in tokens if a!=base)
        for seed in (10**12,10**14):
            initial,amount=prepare(base,seed)
            if not amount:
                refused.append(dict(pool=m["pool"],reason="funding rejected",seed=str(seed)));continue
            for form,spender in [("router",router),("v5",EXEC)]:
                prefix=initial[:];qty=amount
                for a,b in [(base,other),(other,base)]:
                    view=[balance(t,w) for w in (OWNER,EXEC,router) for t in (a,b)]
                    permit=approval(a,spender,qty)
                    quote=tx(m["pool"],"getAmountOut(uint256,address)",["uint256","address"],[qty,a])
                    start=prefix+[permit,quote]+view;i=len(start); loose=order(form,m,a,b,qty,1,deadline)
                    rows=run(start+[loose]+view)
                    assert all(r["status"]=="0x1" for r in rows[:len(prefix)+1]+rows[i-len(view):i]+rows[i+1:])
                    if rows[i]["status"]!="0x1" or rows[len(prefix)+1]["status"]!="0x1":
                        refused.append(dict(pool=m["pool"],wire=form,input=a,qty=str(qty),error=rows[i].get("error")));break
                    old,now=raw(rows[i-len(view):i]),raw(rows[-len(view):])
                    received=now[1]-old[1];spent=old[0]-now[0];quoted=int(rows[len(prefix)+1]["returnData"],16)
                    assert old[2:]==now[2:] and spent==qty and received>0
                    exact=order(form,m,a,b,qty,received,deadline)
                    chk=run(start+[order(form,m,a,b,qty,received+1,deadline),order(form,m,a,b,qty,received,1)]+view+[exact]+view)
                    assert [j for j,r in enumerate(chk) if r["status"]!="0x1"]==[i,i+1]
                    assert raw(chk[i+2:i+2+len(view)])==old and raw(chk[-len(view):])==now
                    result=chk[i+2+len(view)]
                    logs=[x for x in result["logs"] if x["address"]==m["pool"] and x["topics"][0]==SWAP]
                    assert len(logs)==1
                    cases.append(dict(pool=m["pool"],wire=form,input=a,output=b,spent=str(spent),received=str(received),
                                      quoted=str(quoted),gas=int(result["gasUsed"],16),approval_gas=int(chk[len(prefix)]["gasUsed"],16),
                                      tx=exact,logs=result["logs"],refusals=[chk[i]["error"],chk[i+1]["error"]]))
                    prefix+= [permit,exact];qty=received
    return dict(book,cases=cases,unavailable=refused,funding=funding)

