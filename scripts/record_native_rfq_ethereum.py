"""Свежая публичная Native RFQ V6 на обычном текущем head. Котировка запрашивается для отдельного кошелька CI."""
import argparse
import json
import time
from pathlib import Path

import requests
from eth_abi import decode
from eth_utils import keccak
from record_bitswap_ethereum import ENDPOINTS, FUNDING, OWNER, WETH, balance, tx
from record_concave_ethereum import approval

DATA = Path(__file__).resolve().parents[1] / "tests/data/first_run_dex_2026-11-26"
BASE = "https://v2-pub.api.native.org/swap-api-v2/v1/"
EVENT = "0x" + keccak(text="RFQTrade(address,address,address,uint256,uint256,bytes16,address)").hex()
ALLOWED = {"eth_chainId", "eth_getBlockByNumber", "eth_getBlockByHash", "eth_getCode", "eth_getBalance", "eth_simulateV1", "eth_call"}


def read(method, params, until=None):
    if method not in ALLOWED:
        raise ValueError(method)
    errors = []
    for url in ENDPOINTS:
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


def current_offer(src, dst, qty):
    params = dict(src_chain="ethereum", dst_chain="ethereum",token_in=src,token_out=dst,amount_wei=str(qty),from_address=OWNER,version="6")
    res=requests.get(BASE+"firm-quote",params=params,timeout=10)
    res.raise_for_status()
    return res.json()


def leg(roles, src, dst, qty):
    router,pool=[r["address"] for r in roles]
    row=dict(source=src,target=dst,requested=qty)
    offer=current_offer(src,dst,qty)
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
    head=read("eth_getBlockByNumber",["latest",False],end)
    if not head or int(head["timestamp"],16)>=end:return {**row,"status":"expired before simulation"}
    tag={"blockHash":head["hash"],"requireCanonical":True};row["head"]={k:head[k] for k in ("number","hash","timestamp")}
    for role in roles:
        code=read("eth_getCode",[role["address"],tag],end)
        if code is None:return {**row,"status":"expired during checks"}
        if "0x"+keccak(hexstr=code).hex()!=role["runtime"]:return {**row,"status":"changed deployment"}
    owner_code=read("eth_getCode",[OWNER,tag],end);owner_balance=read("eth_getBalance",[OWNER,tag],end)
    if owner_code is None or owner_balance is None:return {**row,"status":"expired during checks"}
    if owner_code!="0x" or int(owner_balance,16)!=0:raise ValueError("адрес исследования уже финансирован")
    fund=tx(WETH,"deposit()",value=qty) if src==WETH else tx(FUNDING,"swapETHForExactTokens(uint256,address[],address,uint256)",["uint256","address[]","address","uint256"],[qty,[WETH,src],OWNER,end],10**17)
    reads=[balance(t,OWNER) for t in (src,dst)]
    order_call={"from":OWNER,"to":router,"data":order["calldata"],"value":"0x0","gas":hex(3_000_000)}
    calls=[fund,approval(src,router,qty),*reads,order_call,*reads]
    child=read("eth_simulateV1",[{"blockStateCalls":[{"stateOverrides":{OWNER:{"balance":hex(10**18)}},"calls":calls}]},tag],end)
    if child is None:return {**row,"status":"expired before simulation"}
    if len(child)!=1 or child[0]["parentHash"]!=tag["blockHash"]:raise ValueError("другой head")
    result=child[0]["calls"];row["child_timestamp"]=int(child[0]["timestamp"],16)
    if len(result)!=len(calls) or any(r["status"]!="0x1" for r in result[:4]):return {**row,"status":"funding unavailable"}
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
    cli=argparse.ArgumentParser(description=__doc__);cli.add_argument("output", type=Path);args=cli.parse_args()
    if read("eth_chainId",[])!="0x1":raise ValueError("сеть")
    fixture=json.loads((DATA/"native_rfq_ethereum_public.json").read_bytes())
    rows=[]
    for sample in fixture["firm_summaries"]:
        q=sample["request"]
        rows.append(leg(fixture["roles"]["roles"],q["token_in"],q["token_out"],int(q["amount_wei"])))
    args.output.write_text(json.dumps(dict(payer=OWNER,source=BASE,observed=rows),separators=(",",":")),encoding="utf8")
