"""Реальные Rubicon Gladius Fill. Читает каноническую квитанцию и вызовы исходной сделки без повторной отправки."""
import argparse
import gzip
import json
import time
from pathlib import Path

import requests
from eth_abi import decode, encode
from eth_utils import keccak

import gladius_trace

ROOT = Path(__file__).resolve().parents[1]
REACTOR = "0x3c53c04d633bec3fb0de3492607c239bf92d07f9"
QUOTER = "0x56e43695d183dcfa9d8fe95e796227a491627fd9"
PERMIT = "0x000000000022d473030f116ddee9f6b43ac78ba3"
ZERO = "0x" + "00" * 20
INFO = "(address,address,uint256,uint256,address,bytes)"
ORDER = f"({INFO},uint256,uint256,address,uint256,(address,uint256,uint256),(address,uint256,uint256,address)[],uint256)"
RESOLVED = f"({INFO},(address,uint256,uint256),(address,uint256,address)[],bytes,bytes32)"
IMPL_SLOT = hex(int.from_bytes(keccak(text="eip1967.proxy.implementation"), "big") - 1)
FILL = "0x" + keccak(text="Fill(bytes32,address,address,uint256)").hex()
TRANSFER = "0x" + keccak(text="Transfer(address,address,uint256)").hex()
# Высоты находят исходные события. Содержимое читается заново по хешу блока.
NETWORKS = {
    "ethereum": (1,26494988,"https://eth.drpc.org", "0xcd4b4242f09f518a18156b6b46a35c5b96a73d3e",
                 [23650582,23678800,23693238,23823071,23823256,23878591,23921564,23949309]),
    "base": (8453,62262756,"https://base.drpc.org", "0x72826cd3c3040e00f2d831d835b1554ec02ef58a",
             [14730222,14730229,14730238,14730244,14730253,14730259,14730266,14730274,14730552,14730559,14730566,14730886]),
}


def sig(s):
    return keccak(text=s)[:4]


def call_data(name, types=(), values=()):
    return "0x" + (sig(name) + encode(types, values)).hex()


def hashed(blob):
    return "0x" + keccak(blob).hex()


RPCS={
    "ethereum":("https://eth.drpc.org","https://mainnet.gateway.tenderly.co","https://ethereum-rpc.publicnode.com"),
    "base":("https://base.drpc.org","https://base.gateway.tenderly.co","https://mainnet.base.org"),
}


class Reader:
    def __init__(self, url):
        self.urls=(url,) if isinstance(url,str) else tuple(url)
        self.preferred={}
        self.http=requests.Session()

    def rpc(self, method, params):
        payload={"jsonrpc":"2.0","id":1,"method":method,"params":params}
        start=self.preferred.get(method,0)
        errors=[]
        for attempt in range(3):
            for offset in range(len(self.urls)):
                idx=(start+offset)%len(self.urls)
                try:
                    response=self.http.post(self.urls[idx],json=payload,timeout=25)
                    response.raise_for_status()
                    result=response.json()
                except (requests.RequestException,ValueError) as err:
                    errors.append(str(err)[:125])
                    continue
                if "error" in result:
                    msg=str(result["error"])
                    if result["error"].get("code")==3 or "execution reverted" in msg.lower():
                        raise RuntimeError(f"{method}: {msg[:180]}")
                    errors.append(msg[:125])
                    continue
                self.preferred[method]=idx
                return result["result"]
            if attempt<2:
                time.sleep(2**attempt)
        raise RuntimeError(f"{method}: RPC unavailable ({'; '.join(errors[-4:])})")

    def view(self, addr, data, at):
        return self.rpc("eth_call", [{"to":addr,"data":data},at])


def addr(word):
    return "0x" + word[-40:].lower()


def canonical(block):
    return {"blockHash":block["hash"],"requireCanonical":True}


def runtime(r, address, at):
    code = bytes.fromhex(r.rpc("eth_getCode",[address,at])[2:])
    assert code
    return hashed(code)


def roles(r, block, controller):
    at=canonical(block)
    result={}
    for role,address in (("reactor",REACTOR),("quoter",QUOTER),("fees",controller)):
        impl=addr(r.rpc("eth_getStorageAt",[address,IMPL_SLOT,at]))
        result[role]={"address":address,"runtime":runtime(r,address,at),"implementation":impl,
                      "implementation_runtime":runtime(r,impl,at) if impl!=ZERO else None}
    assert addr(r.view(REACTOR,call_data("feeController()"),at))==controller
    assert addr(r.view(REACTOR,call_data("permit2()"),at))==PERMIT
    # В Ethereum обратная ссылка равна нулю. Она используется сеттерами ставок;
    # при расчёте выплат код getFeeOutputs её не читает.
    result["fees"]["admin_reactor"]=addr(r.view(controller,call_data("gladiusReactor()"),at))
    result["fees"]["base_fee_units_of_100000"]=int(r.view(controller,call_data("baseFee()"),at),16)
    return result


def nonce_used(r, maker, nonce, at):
    word=int(r.view(PERMIT,call_data("nonceBitmap(address,uint256)",["address","uint256"],[maker,nonce>>8]),at),16)
    return bool(word & (1 << (nonce & 255)))


def extract(r, block, event, tip, archive=None):
    tx_hash=event["transactionHash"]
    tx=r.rpc("eth_getTransactionByHash",[tx_hash])
    receipt=r.rpc("eth_getTransactionReceipt",[tx_hash])
    ix=int(receipt["transactionIndex"],16)
    assert block["transactions"][ix]==tx_hash and tx["blockHash"]==receipt["blockHash"]==event["blockHash"]==block["hash"]
    assert receipt["status"]=="0x1" and any(l==event or (l["logIndex"],l["data"],l["topics"],l["address"])==(event["logIndex"],event["data"],event["topics"],event["address"]) for l in receipt["logs"])
    if archive is None:
        trace=r.rpc("trace_transaction",[tx_hash])
        calls=[v["action"] for v in trace if v["type"]=="call" and v["action"]["callType"]=="call" and not v.get("error")]
    else:
        calls=gladius_trace.archived_calls(tx,archive[tx_hash])
    order_hash,filler,maker=event["topics"][1],addr(event["topics"][2]),addr(event["topics"][3])
    nonce=int(event["data"],16)
    ex="0x"+sig("executeWithCallback((bytes,bytes),bytes)").hex()
    cb="0x"+sig("reactorCallback("+RESOLVED+"[],bytes)").hex()
    orders=[decode(["(bytes,bytes)","bytes"],bytes.fromhex(a["input"][10:]))[0] for a in calls if a["from"]==filler and a["to"]==REACTOR and a["input"].startswith(ex)]
    callbacks=[rec for a in calls if a["from"]==REACTOR and a["to"]==filler and a["input"].startswith(cb) for rec in decode([RESOLVED+"[]","bytes"],bytes.fromhex(a["input"][10:]))[0] if "0x"+rec[-1].hex()==order_hash]
    assert len(orders)==len(callbacks)==1
    order,witness=orders[0]; resolved=callbacks[0]
    info,start,end,exclusive,bps,source,sinks,threshold=decode([ORDER],order)[0]
    stamp=int(block["timestamp"],16)
    assert resolved[0]==info and resolved[3]==witness and info[:3]==(REACTOR,maker,nonce) and stamp<=info[3] and len(sinks)==1
    # Вызов Permit2 содержит именно хеш этой заявки, в дополнение к заполненному nonce.
    assert any(a["to"]==PERMIT and a["from"]==REACTOR and a["input"].startswith("0x137c29fe") and order_hash[2:] in a["input"] for a in calls)
    transfers=[(l["address"],addr(l["topics"][1]),addr(l["topics"][2]),int(l["data"],16)) for l in receipt["logs"] if len(l["topics"])==3 and l["topics"][0]==TRANSFER and len(l["data"])==66]
    transfers += [(ZERO,a["from"],a["to"],int(a["value"],16)) for a in calls if int(a["value"],16)>0]
    token,amount,cap=resolved[1]
    assert token==source[0] and sum(q for t,s,d,q in transfers if (t,s,d)==(token,maker,filler))==amount
    payouts=[]
    for t,q,d in resolved[2]:
        assert archive is None or t!=ZERO  # Урезанная трасса не доказывает нативные платежи.
        payer=REACTOR if t==ZERO else filler
        assert sum(n for a,s,to,n in transfers if (a,s,to)==(t,payer,d))==q
        payouts.append(dict(token=t,raw=str(q),payer=payer,recipient=d))
    historical=canonical(block)
    previous={"blockHash":block["parentHash"],"requireCanonical":True}
    nonces=[nonce_used(r,maker,nonce,v) for v in (previous,historical,tip)]
    assert nonces==[False,True,True]
    quoter_reactor=addr(r.view(QUOTER,call_data("getReactor(bytes)",["bytes"],[order]),previous))
    assert quoter_reactor==REACTOR
    # Квотер запрашивает resolve для собственного caller до callback. У квотера могут быть другие права, чем у filler.
    qdata=call_data("quote(bytes,bytes,uint256)",["bytes","bytes","uint256"],[order,witness,amount])
    quote={"queried":True}
    try:
        candidate=decode([RESOLVED],bytes.fromhex(r.view(QUOTER,qdata,previous)[2:]))[0]
        assert candidate[0]==info and candidate[-1]==resolved[-1]
        quote.update(amount=str(candidate[1][1]),outputs=[[t,str(q),d] for t,q,d in candidate[2]])
    except RuntimeError as err:
        if "RPC unavailable" in str(err):
            raise
        quote["revert"]=str(err)[:200]
    return dict(height=int(block["number"],16),block=block["hash"],timestamp=stamp,index=ix,hash=tx_hash,
                transaction_sender=tx["from"],filler=filler,outer_selector=tx["input"][:10],reactor_selector=ex,
                trace_source="canonical Base archive" if archive is not None else "trace_transaction RPC",
                order_hash=order_hash,witness_digest=hashed(witness),order_digest=hashed(order),maker=maker,
                nonce=str(nonce),used_parent_inclusion_anchor=nonces,deadline=info[3],validation=info[4],
                exclusive=exclusive,override_bps=bps,decay=[start,end],threshold=str(threshold),input_token=token,
                filled=str(amount),cap=str(cap),outputs=payouts,quoter_parent=quote,gas_used=int(receipt["gasUsed"],16))


def record(network,url=None):
    chain,h,_,fee,heights=NETWORKS[network];r=Reader(url or RPCS[network])
    assert int(r.rpc("eth_chainId",[]),16)==chain
    head=r.rpc("eth_getBlockByNumber",[hex(h),False]);tip=canonical(head)
    out=dict(chain=chain,height=h,selector=tip,state_root=head["stateRoot"],roles=roles(r,head,fee),samples=[])
    archive=json.loads(gzip.decompress(gladius_trace.DATA.read_bytes()))["transactions"] if network=="base" else None
    for height in heights:
        block=r.rpc("eth_getBlockByNumber",[hex(height),False])
        logs=r.rpc("eth_getLogs",[{"blockHash":block["hash"],"address":REACTOR,"topics":[FILL]}])
        assert logs
        out["samples"].extend(extract(r,block,l,tip,archive) for l in logs)
    assert r.rpc("eth_getBlockByNumber",[hex(h),False])["hash"]==head["hash"]
    return out


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("network",choices=NETWORKS)
    parser.add_argument("--rpc")
    parser.add_argument("--out",type=Path)
    a=parser.parse_args()
    target=a.out or ROOT/"tests/data/first_run_dex_2026-11-26"/f"gladius_{a.network}_history.json.gz"
    raw=json.dumps(record(a.network,a.rpc),separators=(",",":")).encode()
    target.write_bytes(gzip.compress(raw,mtime=0) if target.suffix==".gz" else raw)
