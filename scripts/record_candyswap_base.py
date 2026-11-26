"""Settle CandySwap's gross pool quote and net wallet quote separately on an archived Base block."""
from eth_abi import decode
from eth_utils import keccak

from record_base_slipstream_executor import EXEC, OWNER, RUNTIME, call, calldata, simulate_block
from record_v2_chain_pools import minimum, swap, view

WETH = "0x4200000000000000000000000000000000000006"
PAYOUT_RATE = "0xc3595a85"


def capture(c, record, header):
    receiver, = c.one(record["factory"], "feeTo()", returns=["address"])
    if receiver in (OWNER, EXEC) or int(receiver,16)==0:
        raise ValueError("unexpected protocol fee receiver")
    deadline = int(header["timestamp"],16)+120
    key = lambda n: "0x"+format(n,"064x")
    state = {OWNER:{"balance":hex(100*10**18)}, EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
             "stateDiff":{key(0):key(int(OWNER,16)),key(1):key(int(WETH,16))}}}

    def balance(token, who):
        return call(token,calldata("balanceOf(address)",["address"],[who]))
    def approve(token, qty):
        return call(token,calldata("approve(address,uint256)",["address","uint256"],[EXEC,qty]))
    def leg(prefix, pool, tokens, reserves, amount, trade_fee, output_fee):
        a,b = tokens
        gross = minimum(amount,*reserves,trade_fee*100)
        cut = gross*output_fee//10000
        net = gross-cut
        if not 0 < net <= gross:
            raise ValueError("invalid payout")
        def order(limit):
            return swap(pool,a,b,amount,limit,deadline,trade_fee*100)
        measured = [balance(a,OWNER),balance(b,OWNER),balance(b,receiver),balance(b,pool)]
        prepared = prefix+[approve(a,amount)]
        replies = simulate_block(c.endpoint,c.selector,state,prepared+measured+[order(net+1),order(net)]+measured)
        offset = len(prepared)
        for i,row in enumerate(replies):
            if i!=offset+4 and row["status"]!="0x1":
                raise ValueError(f"call {i} reverted {str(row.get('error'))[:120]}")
        strict = replies[offset+4]
        if strict["status"]!="0x0":
            raise ValueError("wallet paid above its model")
        before,after=replies[offset:offset+4],replies[offset+6:offset+10]
        deltas=[int(y["returnData"],16)-int(x["returnData"],16) for x,y in zip(before,after)]
        if deltas!=[-amount,net,cut,-gross] or int(replies[offset+5]["returnData"],16)!=net:
            raise ValueError(f"wallet or fee receiver deltas differ: {deltas}")
        return dict(spent=str(amount),gross=str(gross),cut=str(cut),paid=str(net),deltas=list(map(str,deltas)),
                    gas=int(replies[offset+5]["gasUsed"],16),strict=strict.get("error")), prepared+[order(net)]

    rows=[]
    for market in record["pools"]:
        pool=market["pool"]
        code=bytes.fromhex(c.session.post(c.endpoint,json={"id":1,"jsonrpc":"2.0","method":"eth_getCode",
                        "params":[pool,c.selector]},timeout=15).json()["result"][2:])
        # Dispatcher entry c3595a85 returns the exact slot consumed by outbound safeTransfer.
        if ("0x"+keccak(code).hex()!=record["origin"]["pair_runtime"] or
                code[0x7e:0x88]!=bytes.fromhex("63c3595a85146105bf57") or
                code[0x1630:0x1635]!=bytes.fromhex("5b600e5481") or
                code[0x1a5c:0x1a68]!=bytes.fromhex("5b6000612710611a76600e54")):
            raise ValueError("CandySwap outgoing transfer or its getter changed")
        rate,=decode(["uint256"],view(c.endpoint,c.selector,pool,PAYOUT_RATE))
        storage=c.session.post(c.endpoint,json={"id":1,"jsonrpc":"2.0","method":"eth_getStorageAt",
                        "params":[pool,"0xe",c.selector]},timeout=15).json()["result"]
        if rate!=int(storage,16):raise ValueError("outgoing rate differs from storage")
        row=dict(pool=pool,transfer_fee=rate,fee_receiver=receiver,cases=[])
        rows.append(row)
        if WETH not in market["tokens"] or not min(map(int,market["reserves"])):continue
        wi=market["tokens"].index(WETH);token=market["tokens"][1-wi]
        r0,r1=[int(market["reserves"][i]) for i in (wi,1-wi)]
        try:
            for amount in sorted({min(10**15,r0//10000),min(10**16,r0//500)}):
                buy,prefix=leg([call(WETH,calldata("deposit()"),amount)],pool,[WETH,token],[r0,r1],amount,
                               market["commission"]["value"],rate)
                sell,_=leg(prefix,pool,[token,WETH],[r1-int(buy["gross"]),r0+amount],int(buy["paid"]),
                           market["commission"]["value"],rate)
                row["cases"].append(dict(buy=buy,sell=sell))
        except ValueError as error:
            row["reason"]=str(error)[:220]
    if not any(len(row["cases"])==2 for row in rows):raise ValueError("No two complete wallet sizes: "+str([r.get("reason") for r in rows])[:450])
    return rows
