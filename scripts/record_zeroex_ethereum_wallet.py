"""Record expired orders from our own test maker on the canonical Ethereum Settler and AllowanceHolder."""
import argparse
import gzip
import json
import time
from pathlib import Path

import rlp
from eth_abi import encode
from eth_account import Account
from eth_account.messages import encode_typed_data
from eth_utils import keccak

from record_gladius_inventory import PERMIT, SWAPS, TOKENS, Reader, approve, call_data, canonical, fields, funding, tx
from record_gladius_fills import runtime
from zeroex_rfq_format import REGISTRY

ARCHIVE = Path(__file__).resolve().parents[1] / "tests/data/first_run_dex_2026-11-26/zeroex_ethereum_payments.json.gz"
HOLDER = "0x0000000000001ff3684f28c67538d4d072c22734"
USDT = "0xdac17f958d2ee523a2206206994597c13d831ec7"
PERMISSION = "((address,uint256),uint256,uint256)"
SLIPPAGE = "(address,address,uint256)"


def rawcall(method, types, values):
    return bytes.fromhex(call_data(method, types, values)[2:])


def signed_order(maker, actor, settler, src, dst, debit, credit, expiry):
    types = {
        "TokenPermissions": fields("address token,uint256 amount"),
        "Consideration": fields("address token,uint256 amount,address counterparty,bool partialFillAllowed"),
        "PermitWitnessTransferFrom": fields("TokenPermissions permitted,address spender,uint256 nonce,uint256 deadline,Consideration consideration"),
    }
    msg = encode_typed_data(domain_data=dict(name="Permit2", chainId=1, verifyingContract=PERMIT),
        message_types=types, message_data=dict(permitted=dict(token=dst, amount=credit), spender=settler,
            nonce=7, deadline=expiry, consideration=dict(token=src, amount=debit, counterparty=actor, partialFillAllowed=True)))
    return bytes(maker.sign_message(msg).signature)


def original(owner, actor, maker, settler, src, dst, debit, credit, expiry):
    signature = signed_order(maker, actor, settler, src, dst, debit, credit, expiry)
    pull = rawcall("TRANSFER_FROM(address," + PERMISSION + ",bytes)", ["address", PERMISSION, "bytes"],
        [settler, ((src, debit), 0, expiry), b""])
    order = rawcall("RFQ(address," + PERMISSION + ",address,bytes,address,uint256)",
        ["address", PERMISSION, "address", "bytes", "address", "uint256"],
        [settler, ((dst, credit), 7, expiry), maker.address, signature, src, debit])
    settle = rawcall("execute(" + SLIPPAGE + ",bytes[],bytes32)", [SLIPPAGE, "bytes[]", "bytes32"],
        [(owner, dst, credit), [pull, order], bytes(32)])
    return rawcall("exec(address,address,uint256,address,bytes)", ["address","address","uint256","address","bytes"],
        [settler, src, debit, settler, settle])


def record(artifact):
    archive = json.loads(gzip.decompress(ARCHIVE.read_bytes()))
    r = Reader(SWAPS["ethereum"])
    assert int(r.rpc("eth_chainId", []), 16) == 1
    head = r.rpc("eth_getBlockByNumber", [hex(archive["toBlock"]), False])
    tag = canonical(head)
    assert tag == archive["selector"]
    now = int(head["timestamp"], 16) + 1
    expiry = now + 120
    assert time.time() > expiry + 60
    settler = archive["settlers"][0]
    assert r.view(REGISTRY, call_data("ownerOf(uint256)", ["uint256"], [2]), tag)[-40:] == settler[2:]
    roles = {a: runtime(r, a, tag) for a in (REGISTRY, HOLDER, settler)}
    spec = json.loads(artifact.read_bytes())
    creation = "0x" + spec["creation"] + encode(["address","bytes32","address","bytes32"],
        [HOLDER, bytes.fromhex(roles[HOLDER][2:]), REGISTRY, bytes.fromhex(roles[REGISTRY][2:])]).hex()
    code = r.rpc("eth_call", [{"data":creation}, tag])
    assert len(code) > 10
    owner = Account.create().address
    maker = Account.create()
    module = "0x" + keccak(rlp.encode([bytes.fromhex(owner[2:]), b""]))[-20:].hex()
    state = {a: {"balance":hex(10**18)} for a in (owner, maker.address)}
    assert int(r.rpc("eth_getTransactionCount", [owner,tag]),16) == 0
    assert r.rpc("eth_getCode", [module,tag]) == "0x"
    create = {"from":owner,"data":creation,"gas":hex(4_000_000)}
    w, _, router = TOKENS["ethereum"]

    def sim(calls):
        payload = {"blockStateCalls":[{"blockOverrides":{"time":hex(now)},"stateOverrides":state,"calls":calls}],"validation":False}
        block, = r.rpc("eth_simulateV1", [payload, tag])
        assert block["parentHash"] == head["hash"] and int(block["timestamp"],16) == now
        rows = block["calls"]
        assert len(rows) == len(calls) and rows[0]["status"] == "0x1" and rows[0]["returnData"] == code
        return rows

    cases = []
    for src,dst,base_input,base_output in ((w,USDT,10**15,1_600_000),(USDT,w,1_600_000,10**15)):
        for scale in (1,2):
            debit, credit = base_input*scale, base_output*scale
            for mode in ("direct","module"):
                actor = owner if mode=="direct" else module
                spender = HOLDER if mode=="direct" else module
                inputs = [create] + funding(owner,src,w,USDT,router) + funding(maker.address,dst,w,USDT,router)
                inputs += [approve(maker.address,dst,PERMIT,credit),approve(owner,src,spender,debit)]
                observe = [tx(owner,t,call_data("balanceOf(address)",["address"],[a])) for t,a in
                    ((src,owner),(dst,owner),(src,maker.address),(dst,maker.address),(src,module),(dst,module))]
                observe.append(tx(owner,src,call_data("allowance(address,address)",["address","address"],[module,HOLDER])))
                def payment(recipient=owner, counterparty=actor, minimum=credit, deadline=expiry):
                    wire = original(recipient,counterparty,maker,settler,src,dst,debit,credit,expiry)
                    if mode == "direct":
                        return tx(owner,HOLDER,"0x"+wire.hex())
                    return tx(owner,module,call_data("settle(address,address,uint256,uint256,uint256,bytes)",
                        ["address","address","uint256","uint256","uint256","bytes"],[src,dst,debit,minimum,deadline,wire]))
                refusals = [payment(counterparty=maker.address)]
                if mode == "module":
                    refusals += [payment(recipient=maker.address),payment(minimum=credit+1),payment(deadline=now-1)]
                sequence = inputs + observe + refusals + observe + [payment()] + observe + [payment()] + observe
                rows = sim(sequence)
                cursor=0
                def take(count, success):
                    nonlocal cursor
                    part=rows[cursor:cursor+count]
                    cursor+=count
                    for index,row in enumerate(part,cursor-count):
                        if (row["status"]=="0x1") != success:
                            raise ValueError((mode,src,scale,index,row.get("error"),row.get("returnData","")[:75]))
                    return part
                def balances():
                    return [int(row["returnData"],16) for row in take(len(observe),True)]
                setup=take(len(inputs),True)
                before=balances()
                negatives=take(len(refusals),False)
                assert balances()==before
                paid,=take(1,True)
                after=balances()
                repeat,=take(1,False)
                assert balances()==after and cursor==len(rows)
                assert [a-b for a,b in zip(after,before,strict=True)]==[-debit,credit,debit,-credit,0,0,0]
                assert before[4:]==[0,0,0]
                cases.append(dict(mode=mode,input=src,output=dst,debit=str(debit),credit=str(credit),
                    before=list(map(str,before)),after=list(map(str,after)),logs=paid["logs"],gas=int(paid["gasUsed"],16),
                    approvalGas=int(setup[-1]["gasUsed"],16),expiry=expiry,negative=[c["error"] for c in negatives],
                    repeat=repeat["error"],original="0x"+original(owner,actor,maker,settler,src,dst,debit,credit,expiry).hex()))
    return dict(chainId=1,selector=tag,settler=settler,holder=HOLDER,registry=REGISTRY,codeHashes=roles,payer=owner,
        maker=maker.address,module=module,creation=creation,runtime=code,createGas=int(setup[0]["gasUsed"],16),cases=cases)


if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument("artifact",type=Path)
    p.add_argument("out",type=Path)
    args=p.parse_args()
    args.out.write_text(json.dumps(record(args.artifact),separators=(",",":")))
