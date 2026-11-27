"""Истёкшие контрольные ордера двух своих кошельков на оригинальных Textile Reactor и Permit2. Только eth_simulateV1."""
import argparse
import gzip
import json
import time
from pathlib import Path

from eth_abi import encode
from eth_account import Account
from eth_account.messages import encode_typed_data
from eth_utils import keccak

from record_gladius_inventory import PERMIT, SWAPS, TOKENS, ZERO, Reader, approve, call_data, canonical, fields, funding, tx

DATA = Path(__file__).resolve().parents[1] / "tests/data/first_run_dex_2026-11-26/textile_fx.json.gz"
MODULE = "0x0000000000000000000000000000000054455854"
SIGNED = "(bytes,bytes)"
TERMS = "(address,address,uint256,uint256,uint256)"
ORDER = "((address,address,uint256,uint256,address,bytes),(address,uint256,uint256),(address,uint256,address)[])"


def signed(maker, chain, reactor, nonce, deadline, src, dst, sell, buy):
    info = (reactor, maker.address, nonce, deadline, ZERO, b"")
    outputs = [(dst, buy, maker.address)]
    encoded = encode([ORDER], [(info, (src, sell, sell), outputs)])
    types = {
        "OrderInfo": fields("address reactor,address swapper,uint256 nonce,uint256 deadline,address additionalValidationContract,bytes additionalValidationData"),
        "OutputToken": fields("address token,uint256 amount,address recipient"),
        "LimitOrder": fields("OrderInfo info,address inputToken,uint256 inputAmount,OutputToken[] outputs"),
        "TokenPermissions": fields("address token,uint256 amount"),
        "PermitWitnessTransferFrom": fields("TokenPermissions permitted,address spender,uint256 nonce,uint256 deadline,LimitOrder witness"),
    }
    witness = dict(info=dict(zip((k["name"] for k in types["OrderInfo"]), info, strict=True)),
                   inputToken=src, inputAmount=sell, outputs=[dict(token=dst, amount=buy, recipient=maker.address)])
    message = encode_typed_data(domain_data=dict(name="Permit2", chainId=chain, verifyingContract=PERMIT),
        message_types=types, message_data=dict(permitted=dict(token=src, amount=sell), spender=reactor,
        nonce=nonce, deadline=deadline, witness=witness))
    return encoded, bytes(maker.sign_message(message).signature)


def record(network, artifact):
    facts = json.loads(gzip.decompress(DATA.read_bytes()))["chains"][network]
    roles = facts["roles"]
    reactor = roles["reactor"]["address"]
    collector, = facts["fee_recipient"]
    bps, = facts["fee_bps"]
    r = Reader(SWAPS[network])
    chain = {"ethereum": 1, "base": 8453}[network]
    assert int(r.rpc("eth_chainId", []), 16) == chain
    head = r.rpc("eth_getBlockByNumber", [hex(facts["window"][1]), False])
    tag = canonical(head)
    assert tag == facts["selector"] and r.rpc("eth_getCode", [MODULE, tag]) == "0x"
    now = int(head["timestamp"], 16) + 1
    deadline = now + 120
    assert time.time() > deadline + 60
    owner = Account.create().address
    makers = [Account.create(), Account.create()]
    spec = json.loads(artifact.read_bytes())
    creation = spec["creation"] + encode(["address","address","bytes32"],[owner, reactor, bytes.fromhex(roles["reactor"]["runtime"][2:])]).hex()
    runtime = r.rpc("eth_call", [{"data":"0x"+creation},tag])
    assert len(runtime) > 10
    state = {a:{"balance":hex(10**18)} for a in [owner, *(m.address for m in makers)]}
    state[MODULE] = {"code":runtime}
    w, usdc, router = TOKENS[network]

    def sim(ops, stamp=now):
        payload = {"blockStateCalls":[{"blockOverrides":{"time":hex(stamp)},"stateOverrides":state,"calls":ops}],"validation":False}
        child, = r.rpc("eth_simulateV1",[payload,tag])
        assert child["parentHash"] == head["hash"] and int(child["timestamp"],16) == stamp
        return child["calls"]

    result = []
    for src,dst,sell,buy in [(usdc,w,200003,10**14+123),(w,usdc,10**14+101,200003)]:
        for count in (1,2):
            for mode in ("reactor","wallet"):
                actor = owner if mode == "reactor" else MODULE
                setup = funding(owner,dst,w,usdc,router)
                orders = []
                for i,maker in enumerate(makers[:count]):
                    setup += funding(maker.address,src,w,usdc,router) + [approve(maker.address,src,PERMIT,sell+i)]
                    orders.append(signed(maker,chain,reactor,7,deadline,src,dst,sell+i,buy+i))
                amount = sum(sell+i for i in range(count))
                requested = sum(buy+i for i in range(count))
                fee = sum((buy+i)*bps//10000 for i in range(count))
                debit = requested + fee
                setup += [approve(owner,dst,reactor if mode=="reactor" else MODULE,debit)]
                wire = call_data("execute("+SIGNED+")",[SIGNED],orders) if count==1 else call_data("executeBatch("+SIGNED+"[])",[SIGNED+"[]"],[orders])
                def swap(minimum=amount,cap=debit,expiry=deadline):
                    if mode=="reactor":return tx(owner,reactor,wire)
                    return tx(owner,MODULE,call_data("settle("+TERMS+",bytes)",[TERMS,"bytes"],
                        [(dst,src,cap,minimum,expiry),bytes.fromhex(wire[2:])]))
                reads = [tx(owner,t,call_data("balanceOf(address)",["address"],[a])) for t,a in
                         [(dst,owner),(src,owner),(dst,collector),(dst,MODULE),(src,MODULE)]]
                reads += [tx(owner,dst,call_data("allowance(address,address)",["address","address"],[MODULE,reactor]))]
                negative = [] if mode=="reactor" else [swap(amount+1),swap(cap=debit-1),swap(expiry=now-1)]
                results = sim(setup+reads+negative+[swap(),swap()]+reads)
                idx=len(setup); n=len(reads); leg=idx+n+len(negative)
                if any(c["status"]!="0x1" for c in results[:idx+n]+[results[leg]]+results[leg+2:]):
                    raise ValueError(("wallet setup or settlement",network,src,mode,count,[(i,c.get("error")) for i,c in enumerate(results) if c["status"]=="0x0"][:6]))
                assert all(c["status"]=="0x0" for c in results[idx+n:leg]+[results[leg+1]])
                values=lambda rows:[int(row["returnData"],16) for row in rows]
                pre=values(results[idx:idx+n]);post=values(results[leg+2:])
                assert pre[0]-post[0]==debit and post[1]-pre[1]==amount and post[2]-pre[2]==fee and pre[3:]==post[3:]==[0,0,0]
                result.append(dict(mode=mode,orders=count,payer=owner,taker=actor,input=dst,output=src,debit=str(debit),credit=str(amount),fee=str(fee),
                    before=list(map(str,pre)),after=list(map(str,post)),gas=int(results[leg]["gasUsed"],16),approval_gas=int(results[idx-1]["gasUsed"],16),
                    calldata=wire,expiry=deadline,logs=results[leg]["logs"],replay=results[leg+1]["error"],negative=[c["error"] for c in results[idx+n:leg]]))
    return dict(chain=chain,selector=tag,source="expired control orders signed by isolated test makers; no Textile firm API request",roles=roles,
                module=MODULE,owner=owner,makers=[m.address for m in makers],cases=result)


if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__);p.add_argument("network",choices=SWAPS);p.add_argument("artifact",type=Path);p.add_argument("output",type=Path)
    args=p.parse_args();args.output.write_text(json.dumps(record(args.network,args.artifact),separators=(",",":")))
