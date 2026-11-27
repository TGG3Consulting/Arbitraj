"""Собственные истёкшие ордера через защищённый модуль, без публикации в сеть."""
import argparse
import json
from pathlib import Path
import time
from eth_abi import decode, encode
from eth_account import Account
from eth_utils import keccak
from record_gladius_inventory import (FUND, NETWORKS, PERMIT, QUOTER, REACTOR, RESOLVED, SWAPS,
                                      TOKENS, Reader, addr, approve, call_data, canonical, funding,
                                      observe, order, roles, tx)

MODULE = "0x00000000000000000000000000000000474c4144"
LIMIT = "(address,address,uint256,uint256,uint256,uint256)"


def measure(r, head, runtime, maker, payer, calls, stamp):
    state = {MODULE:{"code":runtime}, maker:{"balance":hex(10**18)}, payer:{"balance":hex(10**18)}}
    sim = {"blockStateCalls":[{"blockOverrides":{"time":hex(stamp)}, "stateOverrides":state, "calls":calls}], "validation":False}
    block, = r.rpc("eth_simulateV1",[sim, canonical(head)])
    assert block["parentHash"] == head["hash"] and len(block["calls"]) == len(calls)
    return block["calls"]


def case(r, head, code, chain, collector, maker, payer, src, dst, nonce, partial):
    wrap, usdc, router = TOKENS[chain]
    full, price = (10**14, 200000) if src == wrap else (200000, 10**14)
    quantity = full * 3 // 4 if partial else full
    stamp = int(head["timestamp"],16)+1
    deadline = stamp+100
    assert int(time.time()) > deadline+60
    signed = order(maker,NETWORKS[chain][0],nonce,deadline,src,dst,full,price,full//2)
    setup = funding(maker.address,src,wrap,usdc,router)+funding(payer,dst,wrap,usdc,router)+[
        approve(maker.address,src,PERMIT,full),approve(payer,dst,MODULE,2*price)]
    q = tx(payer,QUOTER,call_data("quote(bytes,bytes,uint256)",["bytes","bytes","uint256"],[*signed,quantity]))
    simulation = lambda calls,t=stamp: measure(r,head,code,maker.address,payer,setup+calls,t)
    call = simulation([q])[-1]
    decoded, = decode([RESOLVED],bytes.fromhex(call["returnData"][2:]))
    debit=sum(v for _,v,_ in decoded[2])
    def action(low,cap,minimum):
        data=call_data("fill("+LIMIT+",(bytes,bytes),uint256)",[LIMIT,"(bytes,bytes)","uint256"],
                       [(dst,src,low,cap,minimum,deadline),signed,quantity])
        return tx(payer,MODULE,data)
    fields=observe(maker.address,payer,collector,src,dst,nonce)+[
        tx(payer,t,call_data("balanceOf(address)",["address"],[MODULE])) for t in (src,dst)]+[
        tx(payer,dst,call_data("allowance(address,address)",["address","address"],[MODULE,REACTOR]))]
    swap=action(debit if partial else 0,debit if partial else debit+7,quantity)
    trials=[action(0,debit-1,quantity),action(0,debit,quantity+1),swap,swap]
    report=simulation(fields+trials+fields)
    n=len(setup);k=len(fields);pre,attempts,after=report[n:n+k],report[n+k:n+k+4],report[n+k+4:]
    assert all(row["status"]=="0x1" for row in report[:n]+pre+after)
    assert [row["status"] for row in attempts]==["0x0","0x0","0x1","0x0"]
    before=[int(x["returnData"],16) for x in pre];after=[int(x["returnData"],16) for x in after]
    assert before[0]-after[0]==after[1]-before[1]==quantity
    assert before[3]-after[3]==debit==(after[2]-before[2])+(after[4]-before[4])
    assert before[6:]==after[6:]==[0,0,0] and not before[5]&(1<<(nonce&255)) and after[5]&(1<<(nonce&255))
    assert simulation([swap],deadline+1)[-1]["status"]=="0x0"
    settled=attempts[2]
    assert decode(["uint256","uint256"],bytes.fromhex(settled["returnData"][2:]))==(debit,quantity)
    return {"quantity":str(quantity),"spent":str(debit),"partial":partial,"before":list(map(str,before)),
            "after":list(map(str,after)),"input":dst,"output":src,"cap":str(debit if partial else debit+7),
            "api_fixture":{"chainId":NETWORKS[chain][0],"orderStatus":"open","encodedOrder":"0x"+signed[0].hex(),
                           "signature":"0x"+signed[1].hex(),"orderHash":"0x"+decoded[-1].hex()},
            "owner":payer,"maker":maker.address,"deadline":deadline,"gas":int(settled["gasUsed"],16),
            "logs":settled["logs"],"input_digest":"0x"+keccak(bytes.fromhex(swap["data"][2:])).hex(),
            "under_cap":attempts[0]["error"],"above_min":attempts[1]["error"],"duplicate":attempts[3]["error"]}


def main(chain, artifact):
    chainid,h,_,ctrl,_=NETWORKS[chain];r=Reader(SWAPS[chain]);assert int(r.rpc("eth_chainId",[]),16)==chainid
    head=r.rpc("eth_getBlockByNumber",[hex(h),False]);at=canonical(head)
    proof=roles(r,head,ctrl); spec=json.loads(artifact.read_bytes())
    creation="0x"+spec["creation"]+encode(["address","bytes32"],[REACTOR,bytes.fromhex(proof["reactor"]["runtime"][2:])]).hex()
    code=r.rpc("eth_call",[{"data":creation},at]);assert len(code)>10 and r.rpc("eth_getCode",[MODULE,at])=="0x"
    collector=addr(r.view(ctrl,call_data("feeRecipient()"),at));w,u,_=TOKENS[chain]
    maker=Account.create();payer=Account.create().address
    cases=[case(r,head,code,chain,collector,maker,payer,src,dst,i+1,part)
           for i,(src,dst,part) in enumerate((a,b,p) for a,b in ((w,u),(u,w)) for p in (False,True))]
    return {"chain":chainid,"height":h,"selector":at,"module":MODULE,"runtime":"0x"+keccak(bytes.fromhex(code[2:])).hex(),
            "roles":proof,"cases":cases,"source":"isolated test-owned expired witnesses; eth_simulateV1 only"}


if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__);p.add_argument("chain",choices=NETWORKS);p.add_argument("artifact",type=Path);p.add_argument("out",type=Path)
    a=p.parse_args();a.out.write_text(json.dumps(main(a.chain,a.artifact),separators=(",",":")))
