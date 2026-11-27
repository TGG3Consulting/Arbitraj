"""Изолированный собственный maker и filler Gladius на опубликованных контрактах. В сеть идут только методы чтения."""
import argparse
import json
import secrets
import time
from pathlib import Path

from eth_abi import decode, encode
from eth_account import Account
from eth_account.messages import encode_typed_data

from record_gladius_fills import FILL, NETWORKS, ORDER, PERMIT, QUOTER, REACTOR, RESOLVED, ZERO, Reader, addr, call_data, canonical, roles

TOKENS = {
    "ethereum": ("0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2", "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
                 "0x68b3465833fb72a70ecdf485e0e4c7bd8665fc45"),
    "base": ("0x4200000000000000000000000000000000000006", "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913",
             "0x2626664c2603336e57b271c5c0b26f421741e481"),
}
FUND = 10**17
SWAPS = {"ethereum":("https://mainnet.gateway.tenderly.co","https://eth.drpc.org","https://ethereum-rpc.publicnode.com"),
         "base":("https://mainnet.base.org","https://base.gateway.tenderly.co","https://base.drpc.org")}


def fields(s):
    return [{"name":field.split()[1],"type":field.split()[0]} for field in s.split(",")]


def order(maker, chain, nonce, deadline, token, payout, size, cost, floor):
    info=(REACTOR,maker.address,nonce,deadline,ZERO,b"")
    start,end=deadline-100,deadline-80
    encoded=encode([ORDER],[(info,start,end,ZERO,0,(token,size,size),[(payout,cost,cost,maker.address)],floor)])
    types={
        "OrderInfo":fields("address reactor,address swapper,uint256 nonce,uint256 deadline,address additionalValidationContract,bytes additionalValidationData"),
        "DutchOutput":fields("address token,uint256 startAmount,uint256 endAmount,address recipient"),
        "GladiusOrder":fields("OrderInfo info,uint256 decayStartTime,uint256 decayEndTime,address exclusiveFiller,uint256 exclusivityOverrideBps,address inputToken,uint256 inputStartAmount,uint256 inputEndAmount,DutchOutput[] outputs,uint256 fillThreshold"),
        "TokenPermissions":fields("address token,uint256 amount"),
        "PermitWitnessTransferFrom":fields("TokenPermissions permitted,address spender,uint256 nonce,uint256 deadline,GladiusOrder witness"),
    }
    witness=dict(info=dict(zip((f["name"] for f in types["OrderInfo"]),info)),
                 decayStartTime=start,decayEndTime=end,exclusiveFiller=ZERO,exclusivityOverrideBps=0,
                 inputToken=token,inputStartAmount=size,inputEndAmount=size,
                 outputs=[dict(token=payout,startAmount=cost,endAmount=cost,recipient=maker.address)],
                 fillThreshold=floor)
    msg=encode_typed_data(domain_data={"name":"Permit2","chainId":chain,"verifyingContract":PERMIT},
                          message_types=types,message_data=dict(permitted=dict(token=token,amount=size),
                          spender=REACTOR,nonce=nonce,deadline=deadline,witness=witness))
    return encoded,bytes(maker.sign_message(msg).signature)


def tx(sender, to, data, value=0):
    return {"from":sender,"to":to,"data":data,"value":hex(value),"gas":hex(1_500_000)}


def approve(sender, token, spender, amount):
    return tx(sender,token,call_data("approve(address,uint256)",["address","uint256"],[spender,amount]))


def funding(payer, token, w, usdc, router):
    calls=[tx(payer,w,call_data("deposit()"),FUND)]
    if token==usdc:
        shape="(address,address,uint24,address,uint256,uint256,uint160)"
        data=call_data("exactInputSingle("+shape+")",[shape],[(w,usdc,500,payer,FUND,1,0)])
        calls.extend([approve(payer,w,router,FUND),tx(payer,router,data)])
    return calls


def observe(maker,filler,collector,src,dst,nonce):
    return [tx(filler,token,call_data("balanceOf(address)",["address"],[owner])) for token,owner in
            ((src,maker),(src,filler),(dst,maker),(dst,filler),(dst,collector))]+[
        tx(filler,PERMIT,call_data("nonceBitmap(address,uint256)",["address","uint256"],[maker,nonce>>8]))]


def simulate(reader, head, sender, filler, calls, stamp):
    state={a:{"balance":hex(10**18)} for a in (sender,filler)}
    block={"blockOverrides":{"time":hex(stamp)},"stateOverrides":state,"calls":calls}
    slots=reader.rpc("eth_simulateV1",[{"blockStateCalls":[block],"validation":False},canonical(head)])
    assert len(slots)==1 and slots[0]["parentHash"]==head["hash"] and int(slots[0]["timestamp"],16)==stamp
    assert len(slots[0]["calls"])==len(calls)
    return slots[0]["calls"]


def example(reader,head,chain,coins,collector,maker,filler,nonce,direction,part):
    w,usdc,router=coins
    src,dst=(w,usdc) if direction=="weth_to_usdc" else (usdc,w)
    total,price=(10**14,200_000) if src==w else (200_000,10**14)
    requested=total if not part else 3*total//4
    threshold=total//2
    now=int(head["timestamp"],16)+1
    deadline=now+120
    # Этот домен и временной интервал уже закрыты в публичной сети. Секреты только у двух новых тестовых сторон.
    assert int(time.time())>deadline+60
    signed=order(maker,chain,nonce,deadline,src,dst,total,price,threshold)
    def execute(q):
        return tx(filler,REACTOR,call_data("execute((bytes,bytes),uint256)",["(bytes,bytes)","uint256"],[signed,q]))
    setup=funding(maker.address,src,w,usdc,router)+funding(filler,dst,w,usdc,router)+[
        approve(maker.address,src,PERMIT,total),approve(filler,dst,REACTOR,2*price)]
    balances=observe(maker.address,filler,collector,src,dst,nonce)
    quote=tx(filler,QUOTER,call_data("quote(bytes,bytes,uint256)",["bytes","bytes","uint256"],[*signed,requested]))
    calls=setup+[quote]+balances+[execute(threshold-1),execute(requested)]+balances+[execute(requested)]+balances
    result=simulate(reader,head,maker.address,filler,calls,now);n=len(setup)
    ok=lambda c:c["status"]=="0x1"
    assert all(ok(c) for c in result[:n+1]), [(j,c.get("error")) for j,c in enumerate(result[:n+1]) if not ok(c)]
    resolved=decode([RESOLVED],bytes.fromhex(result[n]["returnData"][2:]))[0]
    assert resolved[0][0].lower()==REACTOR and resolved[0][1].lower()==maker.address.lower() and resolved[1][1]==requested
    q=[{"token":a,"raw":str(v),"recipient":b} for a,v,b in resolved[2]]
    frame=lambda off:[int(c["returnData"],16) for c in result[off:off+6] if ok(c)]
    before=frame(n+1); after=frame(n+9); repeated=frame(n+16)
    small,swap,duplicate=result[n+7],result[n+8],result[n+15]
    assert not ok(small) and ok(swap) and not ok(duplicate)
    assert len(before)==len(after)==6 and after==repeated
    spent=sum(v for _,v,_ in resolved[2])
    assert (before[0]-after[0],after[1]-before[1],after[2]-before[2],before[3]-after[3],after[4]-before[4])==(
        requested,requested,sum(v for _,v,b in resolved[2] if b.lower()==maker.address.lower()),spent,
        sum(v for _,v,b in resolved[2] if b.lower()==collector))
    assert before[5]&(1<<(nonce&255))==0 and after[5]&(1<<(nonce&255))
    logs=[l for l in swap["logs"] if l["address"].lower()==REACTOR and l["topics"][0]==FILL]
    assert len(logs)==1 and logs[0]["topics"][1]=="0x"+resolved[-1].hex()
    assert addr(logs[0]["topics"][2])==filler.lower() and addr(logs[0]["topics"][3])==maker.address.lower()
    late=simulate(reader,head,maker.address,filler,setup+[execute(requested)]+balances,deadline+1)
    assert all(ok(c) for c in late[:n]) and not ok(late[n])
    assert [int(c["returnData"],16) for c in late[n+1:]]==before
    return dict(direction=direction,partial=part,token=src,payout=dst,order_hash="0x"+resolved[-1].hex(),
                deadline=deadline,fill_threshold=str(threshold),maker_quantity=str(total),actual=str(requested),
                outputs=q,spent=str(spent),before=list(map(str,before)),after=list(map(str,after)),
                gas_used=int(swap["gasUsed"],16),under_threshold=small["error"],repeat=duplicate["error"],expired=late[n]["error"])


def record(network,endpoint):
    chain,h,_,ctrl=NETWORKS[network][:4];r=Reader(endpoint)
    assert int(r.rpc("eth_chainId",[]),16)==chain
    block=r.rpc("eth_getBlockByNumber",[hex(h),False]);at=canonical(block)
    maker=Account.create(secrets.token_bytes(32));filler=Account.create(secrets.token_bytes(32)).address
    for account in (maker.address,filler):
        assert r.rpc("eth_getCode",[account,at])=="0x" and int(r.rpc("eth_getBalance",[account,at]),16)==0
    collector=addr(r.view(ctrl,call_data("feeRecipient()"),at));assert collector!=ZERO
    cases=[example(r,block,chain,TOKENS[network],collector,maker,filler,i+1,direction,part)
           for i,(direction,part) in enumerate((d,p) for d in ("weth_to_usdc","usdc_to_weth") for p in (False,True))]
    return dict(chain=chain,height=h,selector=at,roles=roles(r,block,ctrl),source="isolated owner-signed funding",
                maker=maker.address,filler=filler,fee_recipient=collector,cases=cases)


if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__);p.add_argument("network",choices=NETWORKS);p.add_argument("--rpc");p.add_argument("--out",type=Path,required=True)
    a=p.parse_args();a.out.write_text(json.dumps(record(a.network,a.rpc or SWAPS[a.network]),indent=2))
