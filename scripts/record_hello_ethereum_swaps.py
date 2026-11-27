"""Observe the last complete 24 hours of Hello pool swaps, independently of the pool index."""
import json
from pathlib import Path

from eth_abi import decode
from eth_utils import keccak
from record_balancer_markets import Capture,posts

HERE=Path(__file__).resolve().parents[1]/"tests/data/first_run_dex_2026-11-26"
SWAP="Swap(address,address,int256,int256,uint160,uint128,int24,uint128,uint128)"
COLLECT="CollectProtocol(address,address,uint128,uint128)"

def capture():
    d=json.loads((HERE/"hello_ethereum_directory.json").read_bytes())
    endpoint="https://mainnet.gateway.tenderly.co"
    cap=Capture(dict(d,endpoint=endpoint))
    def rpc(method,params):
        if method not in {"eth_getLogs","eth_getBlockByNumber","eth_getCode","eth_getTransactionReceipt","eth_chainId"}:raise ValueError(method)
        r=cap.session.post(endpoint,json=dict(jsonrpc="2.0",id=1,method=method,params=params),timeout=22)
        r.raise_for_status();return r.json()["result"]
    def header(n):return rpc("eth_getBlockByNumber",[hex(n),False])
    def timestamp(h):return int(h["timestamp"],16)
    end=d["height"]; tip=header(end)
    if int(rpc("eth_chainId",[]),16)!=1 or tip["hash"]!=d["selector"]["blockHash"] or tip["stateRoot"]!=d["state_root"]:raise ValueError("wrong snapshot")
    cutoff=timestamp(tip)-86400
    lo,hi=end-86400,end
    while lo<hi:
        mid=(lo+hi)//2
        if timestamp(header(mid))<=cutoff:lo=mid+1
        else:hi=mid
    start=lo
    if not timestamp(header(start-1))<=cutoff<timestamp(header(start)):raise ValueError("timestamp boundary")
    periphery={
        "router":"0x4a9661f0c80afa93d16a6adb617fa239c1e8fb37","alternate_router":"0x83a7afae53dcb8615879f5b7e348f2bbbdca25d2",
        "quoter":"0x88b0adeb37ed1ad06bc67faeea02d6a76338a0e6","alternate_quoter":"0xdfb2b114a251fbe255788b178692fbae8baa4566",
        "tick_lens":"0x99f4389285021ffaa2d0bd58031629c2f8da60d6"}
    bound={}
    for role,addr in {**periphery,"factory":d["factory"],"deployer":d["deployer"]}.items():
        wire=bytes.fromhex(rpc("eth_getCode",[addr,cap.selector])[2:])
        if not wire:raise ValueError("absent role "+role)
        bound[role]=dict(address=addr,bytes=len(wire),keccak256="0x"+keccak(wire).hex())
        if "router" in role or "quoter" in role:
            result=posts(cap,[(addr,g,[],[]) for g in ("factory()","deployer()","WETH9()")])
            triple=[decode(["address"],v)[0] for v in result]
            if triple!=[d["factory"],d["deployer"],"0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"]:raise ValueError("foreign periphery role")
            bound[role]["factory_deployer_weth"]=triple
    sample=next(m for m in d["markets"] if int(m["liquidity"])>0)
    step=sample["step"]
    current=sample["slot0"][1]//step//256
    words=sorted({current,current-1,current+1,(-(887272//step))//256,(887272//step)//256})
    bits=posts(cap,[(sample["pool"],"tickBitmap(int16)",["int16"],[w]) for w in words])
    word,bitmap=next((w,int.from_bytes(bits_,"big")) for w,bits_ in zip(words,bits,strict=True) if any(bits_))
    ticks,=cap.one(periphery["tick_lens"],"getPopulatedTicksInWord(address,int16)",["address","int16"],[sample["pool"],word],["(int24,int128,uint128)[]"])
    expected={(word*256+i)*step for i in range(256) if bitmap&(1<<i)}
    if {t[0] for t in ticks}!=expected or len(ticks)!=len(expected):raise ValueError("lens missed an initialized tick")
    cells=posts(cap,[(sample["pool"],"ticks(int24)",["int24"],[i]) for i,_,_ in ticks])
    for (_,net,gross),cell in zip(ticks,cells,strict=True):
        if decode(["uint128","int128"],cell[:64])!=(gross,net):raise ValueError("tick gross/net differ from pool")
    bound["tick_lens"].update(sample=sample["pool"],word=word,bitmap=hex(bitmap),ticks=[[i,str(n),str(g)] for i,n,g in ticks])
    markets={r["pool"]:r for r in d["markets"]}
    recipient,=cap.one(d["factory"],"protocolFeeRecipient()",returns=["address"])
    fields=posts(cap,[(a,s,[],[]) for a in markets for s in ["protocolFeeThreshold()","protocolFees()"]])
    fees={}
    for i,a in enumerate(markets):
        t,x=fields[2*i:2*i+2];fees[a]=[str(int.from_bytes(t,"big")),*map(str,decode(["uint128","uint128"],x))]
    ts=["0x"+keccak(text=x).hex() for x in [SWAP,COLLECT]]; totals={};events=[];seen=set();heads={}
    for lower in range(start,end+1,1000):
        query={"address":list(markets),"fromBlock":hex(lower),"toBlock":hex(min(end,lower+999)),"topics":[ts]}
        for log in rpc("eth_getLogs",[query]):
            n=int(log["blockNumber"],16);key=(n,int(log["logIndex"],16));a=log["address"].lower()
            if key in seen or log.get("removed") or a not in markets:raise ValueError("unexpected event")
            if n not in heads:heads[n]=header(n)["hash"]
            if log["blockHash"]!=heads[n]:raise ValueError("orphaned event")
            seen.add(key);events.append(log)
            if log["topics"][0]!=ts[0]:continue
            delta=decode(["int256","int256","uint160","uint128","int24","uint128","uint128"],bytes.fromhex(log["data"][2:]))
            row=totals.setdefault(a,dict(swaps=0,volume_raw=[0,0],protocol_raw=[0,0]))
            row["swaps"]+=1
            for i in (0,1):
                row["volume_raw"][i]+=abs(delta[i]);row["protocol_raw"][i]+=delta[5+i]
    # The protocol emits CollectProtocol after each token transfer; preserve the matching ERC20 movement from the canonical receipt.
    receipts={}; transfers=[];used=set()
    transfer_topic="0x"+keccak(text="Transfer(address,address,uint256)").hex()
    for event in events:
        if event["topics"][0]!=ts[1]:continue
        tx=event["transactionHash"];pool=event["address"];to="0x"+event["topics"][2][-40:]
        if tx not in receipts:receipts[tx]=rpc("eth_getTransactionReceipt",[tx])
        receipt=receipts[tx]
        if receipt["status"]!="0x1" or receipt["blockHash"]!=event["blockHash"] or receipt["transactionHash"]!=tx:raise ValueError("orphaned protocol transfer")
        amounts=decode(["uint128","uint128"],bytes.fromhex(event["data"][2:]))
        for token,amount in zip(markets[pool]["tokens"],amounts,strict=True):
            if not amount:continue
            candidates=[l for l in receipt["logs"] if l["address"]==token and len(l["topics"])==3 and l["topics"][0]==transfer_topic
                        and "0x"+l["topics"][1][-40:]==pool and "0x"+l["topics"][2][-40:]==to and int(l["data"],16)==amount
                        and int(l["logIndex"],16)<int(event["logIndex"],16) and (tx,l["logIndex"]) not in used]
            if len(candidates)!=1:raise ValueError("ambiguous protocol transfer")
            log=candidates[0];used.add((tx,log["logIndex"]));transfers.append(dict(collect_index=event["logIndex"],log=log))
    payouts=[];last_swap={}
    for event in events:
        if event["topics"][0]!=ts[0]:continue
        tx=event["transactionHash"];pool=event["address"];to="0x"+event["topics"][2][-40:]
        if tx not in receipts:receipts[tx]=rpc("eth_getTransactionReceipt",[tx])
        receipt=receipts[tx]
        if receipt["blockHash"]!=event["blockHash"] or receipt["transactionHash"]!=tx or receipt["status"]!="0x1":raise ValueError("different swap receipt")
        signed=decode(["int256","int256"],bytes.fromhex(event["data"][2:])[:64])
        outgoing=[i for i,v in enumerate(signed) if v<0]
        if len(outgoing)!=1:raise ValueError("ambiguous output direction")
        index=outgoing[0];asset=markets[pool]["tokens"][index];end_index=int(event["logIndex"],16)
        matching=[l for l in receipt["logs"] if l["address"]==asset and len(l["topics"])==3 and l["topics"][0]==transfer_topic
                  and "0x"+l["topics"][1][-40:]==pool and "0x"+l["topics"][2][-40:]==to
                  and last_swap.get((tx,pool),-1)<int(l["logIndex"],16)<end_index and (tx,l["logIndex"]) not in used]
        last_swap[(tx,pool)]=end_index
        payouts.append(dict(tx=tx,swap_index=event["logIndex"],pool=pool,asset=asset,recipient=to,quoted=str(-signed[index]),
                            transferred=str(sum(int(l["data"],16) for l in matching)),logs=matching))
    for row in totals.values():
        for f in ["volume_raw","protocol_raw"]:row[f]=list(map(str,row[f]))
    result=dict(chain_id=1,selector=d["selector"],state_root=d["state_root"],from_height=start,to_height=end,cutoff_timestamp=cutoff,end_timestamp=timestamp(tip),protocol_fee_recipient=recipient,
                protocol_transfers=transfers,swap_transfers=payouts,periphery=bound,threshold_current_protocol0_protocol1=fees,markets=totals,events=events,swap_count=sum(r["swaps"] for r in totals.values()))
    if header(end)["hash"]!=tip["hash"]:raise ValueError("orphaned snapshot")
    return result

if __name__=="__main__":
    (HERE/"hello_ethereum_swaps.json").write_text(json.dumps(capture(),indent=2)+"\n",encoding="utf8",newline="\n")
