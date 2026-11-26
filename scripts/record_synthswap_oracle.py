"""Public pool oracle, farming registry and canonical block spacing on SynthSwap Base."""
from eth_utils import keccak

FARM="0x521bc0a04846e67f6933530ad4900146e275eb7f"
CENTER="0x0ca32f1d67d9e805d752aa6250b9e37ac43d2119"

def capture(c,endpoint,pool,gs,roles,header):
    tag=c.selector
    def addr(to,signature,types=(),args=()):
        return c.one(to,signature,types,args,["address"])[0]
    q=lambda name:addr(pool,name)
    active=q("activeIncentive()")
    limit,eternal=c.one(CENTER,"virtualPoolAddresses(address)",["address"],[pool],["address","address"])
    if int(limit,16) or active!=eternal or addr(active,"pool()")!=pool or addr(active,"farmingAddress()")!=FARM or addr(active,"farmingCenterAddress()")!=CENTER:
        raise ValueError("farming registry differs")
    if addr(CENTER,"eternalFarming()")!=FARM or addr(FARM,"farmingCenter()")!=CENTER:
        raise ValueError("farming center changed")
    # Verified AlgebraEternalFarming creates an EternalVirtualPool with CREATE; account nonce 1, RLP d6 94 <address> 01.
    if "0x"+keccak(bytes.fromhex("d694"+FARM[2:]+"01"))[-20:].hex()!=active:
        raise ValueError("unexpected origin for sample incentive")
    roles.update(incentive=active,eternal_farming=FARM,farming_center=CENTER)
    index=gs[3]
    types=["bool","uint32","int56","uint160","uint88","int24","uint144"]
    points=[c.one(pool,"timepoints(uint256)",["uint256"],[i%65536],types) for i in (index-1,index,index+1,index+2,0)]
    raw=c.session.post(endpoint,json={"jsonrpc":"2.0","id":1,"method":"eth_getStorageAt","params":[pool,"0x3",tag]},timeout=20)
    raw.raise_for_status();slot=raw.json()["result"]
    actual,=c.one(pool,"liquidity()",returns=["uint128"])
    if int(slot,16)%(2**128)!=actual:raise ValueError("slot layout changed")
    n=int(header["number"],16);raw=c.session.post(endpoint,json={"jsonrpc":"2.0","id":2,"method":"eth_getBlockByNumber","params":[hex(n+1),False]},timeout=20)
    raw.raise_for_status();child=raw.json()["result"]
    if child["parentHash"]!=header["hash"]:raise ValueError("canonical child changed")
    delta=int(child["timestamp"],16)-int(header["timestamp"],16)
    request=[{"jsonrpc":"2.0","id":i,"method":"eth_getBlockByNumber","params":[hex(n-7+i),False]} for i in range(9)]
    response=c.session.post(endpoint,json=request,timeout=20);response.raise_for_status()
    chain=[r["result"] for r in sorted(response.json(),key=lambda row:row["id"])]
    if chain[-2]["hash"]!=header["hash"] or chain[-1]["hash"]!=child["hash"] or any(b["parentHash"]!=a["hash"] for a,b in zip(chain,chain[1:])):
        raise ValueError("sample interval differs between adjacent canonical blocks")
    changes=[int(b["timestamp"],16)-int(a["timestamp"],16) for a,b in zip(chain,chain[1:])]
    headers=[{key:b[key] for key in ["number","hash","parentHash","timestamp"]} for b in chain]
    if addr(q("factory()"),"farmingAddress()")!=CENTER:raise ValueError("factory no longer lists the center")
    horizon,chunk=8,4
    offsets=list(range(horizon+delta+1));starts=[]
    for i in range(0,len(offsets),chunk):
        before=[86400-t for t in offsets[i:i+chunk]]
        t,_,v,q=c.one(pool,"getTimepoints(uint32[])",["uint32[]"],[before],["int56[]","uint160[]","uint112[]","uint256[]"])
        starts.extend(zip(t,v,q,strict=True))
    row=gs[1]//256
    observed={}
    for word in [row-1,row,row+1]:
        bitmap,=c.one(pool,"tickTable(int16)",["int16"],[word],["uint256"])
        for bit in range(256):
            if bitmap&(1<<bit):
                tick=word*256+bit
                answer=c.one(pool,"ticks(int24)",["int24"],[tick],["uint128","int128","uint256","uint256","int56","uint160","uint32","bool"])
                if not answer[0] or not answer[-1]:raise ValueError("unbacked raw tick")
                observed[str(tick)]=[str(answer[0]),str(answer[1])]
    spacing,=c.one(pool,"tickSpacing()",returns=["int24"])
    bound=(887272//spacing)*spacing
    boundary_words={}
    for tick in [-bound,bound]:
        bitmap,=c.one(pool,"tickTable(int16)",["int16"],[tick//256],["uint256"])
        boundary_words[str(tick//256)]=str(bitmap)
    return dict(timestamp=int(header["timestamp"],16),child_hash=child["hash"],block_step=delta,horizon=horizon,
        headers=headers,observed_steps=changes,chunk=chunk,slot=slot,timepoints=points,starts=starts,create_nonce=1,ticks=observed,boundary_words=boundary_words,words=[row-1,row+1])
