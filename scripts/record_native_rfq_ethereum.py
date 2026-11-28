"""Свежая публичная Native RFQ V6 на обычном текущем head. Котировка запрашивается для отдельного кошелька CI."""
import argparse
import json
import time
from pathlib import Path

import requests
from eth_abi import decode, encode
from eth_utils import keccak
from record_bitswap_ethereum import ENDPOINTS, FUNDING, OWNER, WETH, balance, tx
from record_concave_ethereum import approval

DATA = Path(__file__).resolve().parents[1] / "tests/data/first_run_dex_2026-11-26"
BASE = "https://v2-pub.api.native.org/swap-api-v2/v1/"
EVENT = "0x" + keccak(text="RFQTrade(address,address,address,uint256,uint256,bytes16,address)").hex()
ALLOWED = {"eth_chainId", "eth_getBlockByNumber", "eth_getBlockByHash", "eth_getCode", "eth_getBalance", "eth_getTransactionCount", "eth_simulateV1", "eth_call"}


def read(method, params, until=None, *, endpoints=ENDPOINTS):
    if method not in ALLOWED:
        raise ValueError(method)
    errors = []
    for url in endpoints:
        if until is not None and time.time() >= until:
            return None
        try:
            res = requests.post(url, json={"jsonrpc":"2.0","id":1,"method":method,"params":params}, timeout=12)
            res.raise_for_status()
            reply = res.json()
            if "result" in reply:
                return reply["result"]
            errors.append(str(reply.get("error"))[:160])
        except (KeyError,ValueError,requests.RequestException) as err:
            errors.append(str(err)[:160])
    raise RuntimeError((method, errors))


def positive_int(value):
    value=int(value)
    if value<=0: raise ValueError("требуется положительная сумма")
    return value


def current_offer(src, dst, qty, slug):
    params = dict(src_chain=slug, dst_chain=slug,token_in=src,token_out=dst,amount_wei=str(qty),from_address=OWNER,beneficiary_address=OWNER,allow_multihop="false",version="6")
    res=requests.get(BASE+"firm-quote",params=params,timeout=10)
    res.raise_for_status()
    return res.json()


def protected_calls(artifact, roles, src, dst, qty, minimum, deadline, signed, fund, strict=None):
    target, market = roles
    # Первый вызов создаёт только собственный временный контракт внутри child, исходный nonce проверяется отдельно.
    module = "0x" + keccak(bytes.fromhex("d694" + OWNER[2:] + "80"))[-20:].hex()
    constructor = encode(["address","bytes32","address","bytes32"],
        [target["address"], bytes.fromhex(target["runtime"][2:]), market["address"], bytes.fromhex(market["runtime"][2:])])
    create = {"from": OWNER, "data": "0x" + artifact["creation"] + constructor.hex(), "gas": hex(8_000_000)}
    args = ["address","address","uint256","uint256","uint256","bytes"]
    def exchange(limit, bound):
        return tx(module, "execute(address,address,uint256,uint256,uint256,bytes)", args, [src,dst,qty,limit,bound,signed])
    checks = [balance(t,w) for w in (OWNER,module) for t in (src,dst)]
    checks += [tx(src,"allowance(address,address)",["address","address"],[module,target["address"]])]
    before = [create, fund, approval(src,module,qty), *checks]
    controls = [exchange(2**256-1 if strict is None else strict, deadline), exchange(minimum, 0)]
    op = exchange(minimum, deadline)
    return before + controls + checks + [op] + checks, len(before), len(checks), module


def leg(roles, src, dst, qty, artifact=None, *, slug='ethereum', reader=read, funding=None):
    router,pool=[r["address"] for r in roles]
    row=dict(source=src,target=dst,requested=qty)
    offer=current_offer(src,dst,qty,slug)
    if offer.get("success") is not True or len(offer.get("orders",[]))!=1:
        return {**row,"status":"no single order","reason":offer.get("message") or offer.get("errorMessage")}
    o=offer["orders"][0];order=offer["txRequest"];recipient=OWNER.lower()
    if o.get("externalSwapCalldata") or offer.get("fallbackSwapDataArray") or o.get("multiHop") or int(offer['widgetFee']['feeRate']) or int(o['widgetFee']['feeRate']):
        return {**row,"status":"external leg or fee"}
    terms=(str(offer.get("router_version")),order["target"].lower(),order.get("function"),o["pool"].lower(),
           o["recipient"].lower(),o["sellerToken"].lower(),o["buyerToken"].lower(),positive_int(o["sellerTokenAmount"]))
    if terms!=("6",router,"tradeRFQT",pool,recipient,src,dst,qty) or str(order["value"])!="0" or not order["calldata"].startswith("0x7083527c"):
        return {**row,"status":"different order"}
    signed=bytes.fromhex(order["calldata"][2:])
    if len(signed)<4+12*32:return {**row,"status":"unknown V6 shape"}
    word=lambda i:int.from_bytes(signed[4+32*i:36+32*i],"big")
    minimum=positive_int(o["amountOutMinimum"]);end=positive_int(o["deadlineTimestamp"])
    if [word(i) for i in range(3,12)] != [int(pool,16),int(o["signer"],16),int(recipient,16),int(src,16),int(dst,16),qty,positive_int(o["buyerTokenAmount"]),minimum,end]:
        return {**row,"status":"unknown V6 layout"}
    row.update(quoteId=o["quoteId"],maker=o["signer"],minimum=minimum,deadline=end,offer_digest="0x"+keccak(signed).hex())
    # Только последняя опубликованная голова во время действия полученного предложения.
    head=reader("eth_getBlockByNumber",["latest",False],end)
    if not head or int(head["timestamp"],16)>=end:return {**row,"status":"expired before simulation"}
    tag={"blockHash":head["hash"],"requireCanonical":True};row["head"]={k:head[k] for k in ("number","hash","timestamp")}
    for role in roles:
        code=reader("eth_getCode",[role["address"],tag],end)
        if code is None:return {**row,"status":"expired during checks"}
        if "0x"+keccak(hexstr=code).hex()!=role["runtime"]:return {**row,"status":"changed deployment"}
    owner_code=reader("eth_getCode",[OWNER,tag],end);owner_balance=reader("eth_getBalance",[OWNER,tag],end)
    if owner_code is None or owner_balance is None:return {**row,"status":"expired during checks"}
    if owner_code!="0x" or int(owner_balance,16)!=0:raise ValueError("адрес исследования уже финансирован")
    fund=tx(WETH,"deposit()",value=qty) if src==WETH else tx(FUNDING,"swapETHForExactTokens(uint256,address[],address,uint256)",["uint256","address[]","address","uint256"],[qty,[WETH,src],OWNER,end],10**17)
    if funding is not None:
        fund=funding(src,qty,end)
    reads=[balance(t,OWNER) for t in (src,dst)]
    order_call={"from":OWNER,"to":router,"data":order["calldata"],"value":"0x0","gas":hex(3_000_000)}
    calls=[fund,approval(src,router,qty),*reads,order_call,*reads]
    if artifact is not None:
        own_nonce=reader("eth_getTransactionCount",[OWNER,tag],end)
        if own_nonce is None:return {**row,"status":"expired before constructor"}
        if int(own_nonce,16)!=0:raise ValueError("ненулевой исходный nonce собственного адреса")
        calls, controls, width, module = protected_calls(artifact,roles,src,dst,qty,minimum,end,signed,fund)
    child=reader("eth_simulateV1",[{"blockStateCalls":[{"stateOverrides":{OWNER:{"balance":hex(10**18)}},"calls":calls}]},tag],end)
    if child is None:return {**row,"status":"expired before simulation"}
    if len(child)!=1 or child[0]["parentHash"]!=tag["blockHash"]:raise ValueError("другой head")
    result=child[0]["calls"];row["child_timestamp"]=int(child[0]["timestamp"],16)
    if artifact is not None:
        if len(result)!=len(calls):raise ValueError("неполная симуляция модуля")
        if any(r["status"]!="0x1" for r in result[:controls]):
            return {**row,"status":"module or funding unavailable","reason":[r.get("error") for r in result[:controls] if r["status"]!="0x1"]}
        if result[controls]["status"]=="0x1":raise ValueError("предельный минимум не остановил вызов")
        if result[controls+1]["status"]=="0x1":raise ValueError("личный срок не остановил вызов")
        index=controls+2+width
        swap=result[index]
        if swap["status"]!="0x1":return {**row,"status":"protected call reverted","reason":swap.get("error")}
        def balances(part):
            if any(r["status"]!="0x1" or len(r["returnData"])!=66 for r in part):raise ValueError("чтение средств")
            return [int(r["returnData"],16) for r in part]
        pre=balances(result[controls-width:controls]); rollback=balances(result[controls+2:index]); post=balances(result[index+1:])
        if pre!=rollback or pre[2:]!=post[2:] or pre[-1]!=0:raise ValueError("остатки/allowance модуля")
        module_topic="0x"+keccak(text="Settled(bytes16,address,address,bytes32,address,address,uint256,uint256)").hex()
        events=[l for l in swap["logs"] if l["address"].lower()==module and l["topics"][:1]==[module_topic]]
        qid=bytes.fromhex(o["quoteId"].removeprefix("0x"))
        topics=[module_topic,"0x"+qid.hex()+"00"*16,"0x"+("00"*12)+recipient[2:],"0x"+("00"*12)+o["signer"].lower()[2:]]
        if len(events)!=1 or events[0]["topics"]!=topics:raise ValueError("событие модуля")
        dg,si,so,sp,nt=decode(["bytes32","address","address","uint256","uint256"],bytes.fromhex(events[0]["data"][2:]))
        if (dg,si,so,sp,nt)!=(keccak(signed),src,dst,qty,post[1]-pre[1]):raise ValueError("денежное событие модуля")
        # Измеренный выход может быть выше подписанного минимума. На той же ещё текущей голове
        # проверяем именно чистый приход + 1, с действующей котировкой и обычным временем узла.
        live=reader("eth_getBlockByNumber",["latest",False],end)
        if not live or live["hash"]!=head["hash"]:return {**row,"status":"current head advanced before strict bound"}
        strict_calls,_,_,_=protected_calls(artifact,roles,src,dst,qty,minimum,end,signed,fund,nt+1)
        strict_result=reader("eth_simulateV1",[{"blockStateCalls":[{"stateOverrides":{OWNER:{"balance":hex(10**18)}},"calls":strict_calls}]},tag],end)
        if not strict_result:return {**row,"status":"expired before strict bound"}
        if len(strict_result)!=1 or strict_result[0]["parentHash"]!=head["hash"] or strict_result[0]["timestamp"]!=child[0]["timestamp"]:
            return {**row,"status":"current child advanced before strict bound"}
        second=strict_result[0]["calls"]
        if len(second)!=len(result) or any(r["status"]!="0x1" for r in second[:controls]) or second[controls]["status"]!="0x0" or second[index]["status"]!="0x1":
            return {**row,"status":"strict bound not reproduced"}
        if balances(second[controls+2:index])!=pre or balances(second[index+1:])!=post:raise ValueError("иная выплата при проверке minimum+1")
        row.update(module=module, runtime="0x"+keccak(hexstr=result[0]["returnData"]).hex(),
                   module_before=pre[2:],module_after=post[2:],strict=nt+1,rollback=rollback)
        result=[result[1],result[2],*result[controls-width:controls-width+2],swap,*result[index+1:index+3]]
    if len(result)!=7 or any(r["status"]!="0x1" for r in result[:4]):return {**row,"status":"funding unavailable"}
    swap=result[4]
    if swap["status"]!="0x1":return {**row,"status":"external revert","reason":swap.get("error")}
    if row["child_timestamp"]>end or any(r["status"]!="0x1" for r in result[5:]):raise ValueError("срок или остатки")
    pre=[int(v["returnData"],16) for v in result[2:4]];post=[int(v["returnData"],16) for v in result[5:]]
    paid,net=pre[0]-post[0],post[1]-pre[1]
    events=[l for l in swap["logs"] if l["address"].lower()==pool and l["topics"]==[EVENT]]
    if len(events)!=1:return {**row,"status":"ambiguous event","paid":paid,"net":net}
    recipient,token_in,token_out,sold,bought,quote_id,signer=decode(["address","address","address","uint256","uint256","bytes16","address"],bytes.fromhex(events[0]["data"][2:]))
    if (recipient,token_in,token_out,sold,bought,quote_id.hex(),signer)!=(OWNER.lower(),src,dst,qty,net,o["quoteId"].lower(),o["signer"].lower()) or paid!=qty or net<minimum:
        return {**row,"status":"unreconciled","paid":paid,"net":net}
    return {**row,"status":"settled","paid":paid,"net":net,"before":pre,"after":post,"gas":int(swap["gasUsed"],16),
            "approve_gas":int(result[1]["gasUsed"],16),"logs":swap["logs"]}


if __name__=="__main__":
    cli=argparse.ArgumentParser(description=__doc__);cli.add_argument("output", type=Path);cli.add_argument("--module",type=Path);args=cli.parse_args()
    if read("eth_chainId",[])!="0x1":raise ValueError("сеть")
    fixture=json.loads((DATA/"native_rfq_ethereum_public.json").read_bytes())
    artifact=json.loads(args.module.read_bytes()) if args.module else None
    rows=[]
    for sample in fixture["firm_summaries"]:
        q=sample["request"]
        for attempt in range(3):
            sample_result=leg(fixture["roles"]["roles"],q["token_in"],q["token_out"],int(q["amount_wei"]),artifact)
            if sample_result["status"] not in {"current head advanced before strict bound","current child advanced before strict bound"}:break
        rows.append(sample_result)
    args.output.write_text(json.dumps(dict(payer=OWNER,source=BASE,observed=rows),separators=(",",":")),encoding="utf8")
