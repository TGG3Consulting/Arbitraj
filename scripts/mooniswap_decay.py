"""Канонические котировки сразу после настоящего Swap и в следующих блоках без нового обмена."""
from eth_abi import decode
from record_llama_base_cl import CanonicalCalls

def observe(reader,header,event,successor,aggregate,multi):
    pool=event["pool"]
    tag={"blockHash":header["hash"],"requireCanonical":True}
    cap=CanonicalCalls(reader)
    cap.selector=tag
    a,b=event["target"],event["source"]
    qty=int(event["output"])
    ops=[(pool,"fee()",[],[]),(pool,"decayPeriod()",[],[]),(pool,"getReturn(address,address,uint256)",["address","address","uint256"],[a,b,qty])]
    if successor:
        ops.append((pool,"slippageFee()",[],[]))
    for asset in (a,b):
        if int(asset,16)==0:
            ops.append((multi,"getEthBalance(address)",["address"],[pool]))
        else:
            ops.append((asset,"balanceOf(address)",["address"],[pool]))
        for name in ("virtualBalancesForAddition","virtualBalancesForRemoval"):
            ops.append((pool,name+"(address)",["address"],[asset]))
    raw=aggregate(cap,ops)
    if any(v is None for v in raw):
        return None
    it=iter(raw)
    word=lambda:str(int.from_bytes(next(it),"big"))
    virtual=lambda:list(map(str,decode(["uint216","uint40"],next(it))))
    result=dict(header=header["hash"],number=int(header["number"],16),timestamp=int(header["timestamp"],16),
                fee=word(),decay=word(),quote=word(),slippage_fee=word() if successor else "0",
                balances=[],addition=[],removal=[])
    for _ in range(2):
        result["balances"].append(word())
        result["addition"].append(virtual())
        result["removal"].append(virtual())
    return result

def capture(reader,events,targets,successor,aggregate,multi):
    for event in reversed(events):
        if event["pool"] not in targets: continue
        header=reader.one("eth_getBlockByHash",[event["hash"],False])
        first=observe(reader,header,event,successor,aggregate,multi)
        if not first or not int(first["decay"]):continue
        last_offset=1+int(first["decay"])//12
        if first["number"]+last_offset>int(reader.header["number"],16):continue
        frames=[first]
        for offset in sorted({1,max(2,last_offset//2),last_offset}):
            h=reader.one("eth_getBlockByNumber",[hex(first["number"]+offset),False])
            frame=observe(reader,h,event,successor,aggregate,multi)
            if not frame or any(frame[k]!=first[k] for k in ("balances","addition","removal","fee","slippage_fee","decay")):break
            frames.append(frame)
        if len(frames)==4 and frames[-1]["timestamp"]>=first["timestamp"]+int(first["decay"]) and len({r["quote"] for r in frames})>=3:
            return dict(event=event,input=event["target"],output=event["source"],amount=event["output"],frames=frames)
    raise ValueError("не найден нетронутый интервал с тремя различными котировками")
