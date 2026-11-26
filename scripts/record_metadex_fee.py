"""Read the same head, oracle observations and future first-swap fees as the Aero adapter."""
from eth_abi import decode
from record_balancer_markets import Capture
from record_base_slipstream_executor import calldata, rpc, EXEC


def fees(e, pool, hook, factory, quoter, block, spacing, ahead):
    p, h = pool, hook
    cap=Capture(e)
    def single(to, f, t=(), args=(), out=("uint256",)):
        return cap.one(to,f,t,args,out)
    row={}
    for name in ("secondsAgo","defaultScalingFactor","defaultFeeCap"):
        row[name]=single(h,name+"()")[0]
    cfg=single(h,"dynamicFeeConfig(address)",["address"],[p],["uint24","uint24","uint64","bool","uint24"])
    row["config"]=list(cfg)
    row["tick_fee"]=single(factory,"tickSpacingToFee(int24)",["int24"],[spacing],["uint24"])[0]
    row["stored"]=single(h,"blockFee(address,uint256)",["address","uint256"],[p,e["block"]])[0]
    state=single(p,"slot0()",out=["uint160","int24","uint16","uint16","uint16","bool"])
    row["sqrt"]=str(state[0]);row["tick"]=state[1];row["cardinality"]=state[3]
    if not state[3]:raise ValueError("empty oracle")
    for field,idx in (("oldest",(state[2]+1)%state[3]),("zero",0)):
        obs=single(p,"observations(uint256)",["uint256"],[idx],["uint32","int56","uint160","bool"])
        row[field]=[obs[0],obs[3]]
    window=row["secondsAgo"]
    cums=[];queries=[(p,"observe(uint32[])",["uint32[]"],[[window-d,0]]) for d in range(min(ahead,window)+1)]
    for j in range(0,len(queries),3):
        for raw in cap.batch(queries[j:j+3]):
            cums.append(list(decode(["int56[]","uint160[]"],raw)[0]) if raw else None)
    row["cums"]=cums
    row["probes"]=[]
    for seconds in sorted({1,ahead//2,ahead}):
        calls=[{"from":caller,"to":p,"data":calldata("fee()")} for caller in (quoter,EXEC)]
        q=dict(blockStateCalls=[dict(blockOverrides={"time":hex(block+seconds)},calls=calls)])
        answer=rpc(e["endpoint"],"eth_simulateV1",[q,e["selector"]])[0]["calls"]
        if any(r["status"]!="0x1" for r in answer): raise ValueError("fee() rejected")
        row["probes"].append(dict(seconds=seconds,requests=calls,results=answer))
    row["reads"]=cap.calls
    return row
