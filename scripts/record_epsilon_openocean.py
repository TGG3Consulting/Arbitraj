"""Архив двух публичных способов оплатить OpenOcean. Все операции живут внутри одного дочернего eth_simulateV1."""
import argparse
import json
from pathlib import Path
import time
import requests
from eth_abi import decode, encode
from eth_utils import keccak
from record_epsilon_base import OWNER, PROVIDERS, ROUTER, USDC, WETH, UINT, ZERO, call, data, leg, method, read, rpc

OCEAN = "0x6352a56caadc4f1e25cd6c75970fa768a3304e64"
PREPAID = "0x114b11ae066a7785452c4570b42fff1b7fa5b001"
EXEC = "0x9cafca2507a6128931f88ccc3d83d8bcf74b09f0"
WALLET = "0x" + keccak(text="Epsilon public OpenOcean terminal archive")[-20:].hex()
API = "https://open-api.openocean.finance/v4/base/swap"
ORDER = "(address,address,uint256,uint256,uint256,uint16,bool)"
DESC = "(address,address,address,address,uint256,uint256,uint256,uint256,address,bytes)"
CALLS = "(uint256,uint256,uint256,bytes)[]"


def firm(src, dst, n, caller):
    args = dict(inTokenAddress=src, outTokenAddress=dst, amountDecimals=str(n), gasPriceDecimals="10000000",
                slippage="0.5", sender=caller, account=caller)
    response = requests.get(API, params=args, timeout=15)
    response.raise_for_status()
    doc = response.json()
    if doc.get("code") != 200:
        raise ValueError(("API", str(doc.get("message"))[:160]))
    d = doc["data"]
    raw = bytes.fromhex(d["data"].removeprefix("0x"))
    if (int(d["chainId"]), d["from"].lower(), d["to"].lower(), int(d["value"]), raw[:4].hex()) != (8453, caller, OCEAN, 0, "90411a32"):
        raise ValueError("unexpected provider target, funding or method")
    params = ["address", DESC, CALLS]
    executor, t, calls = decode(params, raw[4:])
    if ((executor, t[:6], t[7:10]) != (EXEC, (src, dst, t[2], caller, n, int(d["minOutAmount"])), (2, ZERO, b""))
            or not calls or not t[5] or encode(params, [executor, t, calls]) != raw[4:]):
        raise ValueError("provider returned another kind of payment")
    if t[2]!=EXEC:
        if t[2]!=PREPAID or calls[0][0]!=int(PREPAID,16) or calls[0][2]!=0 or calls[0][3][:4]!=bytes.fromhex("d6531cdc"):
            raise ValueError("unverified source receiver")
        transfer=decode(["address","address","uint256","address","bytes"],calls[0][3][4:])
        if (transfer[0],transfer[1],transfer[3])!=(src,dst,EXEC):
            raise ValueError("unaccepted source-to-provider payment")
    return dict(funding=t[2],data=d["data"], block=int(d["blockNumber"]), minimum=str(t[5]), expected=str(d["outAmount"]))


def latest_header(endpoint):
    response=requests.post(endpoint,json={"jsonrpc":"2.0","id":1,"method":"eth_getBlockByNumber","params":["latest",False]},timeout=15)
    response.raise_for_status()
    block=response.json().get("result")
    if not isinstance(block,dict) or not block.get("hash"):
        raise ValueError("public latest header unavailable")
    return block


def capture(url, artifact):
    live = latest_header(url)
    if int(rpc(url, "eth_chainId", []), 16) != 8453:
        raise ValueError("chain")
    tag = {"blockHash": live["hash"], "requireCanonical": True}
    fee = int.from_bytes(read(url, [method(ROUTER, "protocolFeeBps()"), tag]), "big")
    offers = []
    for src, dst, sizes in [(WETH, USDC, [10**15, 4*10**15]), (USDC, WETH, [10**6, 4*10**6])]:
        for n in sizes:
            for nested in [False, True]:
                caller = ROUTER if nested else WALLET
                net = n - n*fee//10000 if nested else n
                try:
                    plan = firm(src, dst, net, caller)
                except (requests.RequestException, ValueError, KeyError) as exc:
                    plan = {"refused": str(exc)[:220]}
                offers.append(dict(src=src, dst=dst, n=str(n), nested=nested, firm=plan))
                time.sleep(0.25)
    latest = latest_header(url)
    head = latest["hash"]; tag = {"blockHash": head, "requireCanonical": True}
    ts = int(latest["timestamp"], 16) + 2
    if any(rpc(url, "eth_getCode", [a, tag]) != "0x" for a in [OWNER, WALLET]):
        raise ValueError("experimental addresses already occupied")
    init = artifact["creation"].removeprefix("0x") + encode(["address"]*4, [ROUTER, OCEAN, EXEC, PREPAID]).hex()
    overrides = {OWNER: {"balance": hex(10**18)}}
    runtime = read(url, [{"from": OWNER, "data": "0x" + init}, tag, overrides])
    template = bytes.fromhex(artifact["runtime"].removeprefix("0x"))
    masked = bytearray(runtime)
    for refs in artifact["immutableReferences"].values():
        seen = set()
        for ref in refs:
            start, size = ref["start"], ref["length"]
            seen.add(runtime[start:start+size])
            masked[start:start+size] = template[start:start+size]
        if len(seen) != 1:
            raise ValueError("immutable identity")
    if masked != template:
        raise ValueError("runtime")
    overrides[WALLET] = {"code": "0x" + runtime.hex()}
    def approve(coin, spender, qty):
        return method(coin, "approve(address,uint256)", ["address", UINT], [spender, qty])
    def one(o, floor, end):
        src, dst, n, nested = o["src"], o["dst"], int(o["n"]), o["nested"]
        fund = [method(WETH, "deposit()", value=2*10**16)]
        if src == USDC:
            fund += [approve(WETH, PROVIDERS[1], 10**16),
                     call(PROVIDERS[1], "0x"+leg(WETH, USDC, 500, OWNER, 10**16, 1, ts+60).hex())]
        prefix = fund + [approve(src, WALLET, n)]
        reads = [method(t, "balanceOf(address)", ["address"], [a]) for a in (OWNER, ROUTER, WALLET) for t in (src, dst)]
        reads += [method(src, "allowance(address,address)", ["address","address"], [WALLET, ROUTER if nested else OCEAN])]
        payload = data("trade("+ORDER+",bytes)", [ORDER, "bytes"], [(src,dst,n,floor,end,fee if nested else 0,nested),bytes.fromhex(o["firm"]["data"][2:])])
        calls = prefix + reads + [call(WALLET, payload, gas=7000000)] + reads
        [child] = rpc(url,"eth_simulateV1",[{"blockStateCalls":[{"blockOverrides":{"time":hex(ts)},"stateOverrides":overrides,"calls":calls}]},tag])
        if child["parentHash"] != head or len(child["calls"]) != len(calls):
            raise ValueError("parent or calls")
        rows = child["calls"]; i = len(prefix)+len(reads); result = rows[i]
        if any(x["status"]!="0x1" for x in rows[:i]+rows[i+1:]):
            raise ValueError("funding or balance read")
        vals = lambda arr: [int(x["returnData"],16) for x in arr]
        b,a=vals(rows[len(prefix):i]),vals(rows[i+1:])
        if b[2:]!=a[2:]:raise ValueError("router, module or allowance changed")
        return dict(status=result["status"],error=result.get("error"),gas=int(result["gasUsed"],16),approvalGas=int(rows[len(prefix)-1]["gasUsed"],16),
                    spent=str(b[0]-a[0]),received=str(a[1]-b[1]),before=list(map(str,b)),after=list(map(str,a)),logs=result["logs"],data=payload)
    for o in offers:
        if "data" not in o["firm"]:continue
        loose = one(o,1,ts+60);o["loose"]={k:loose[k] for k in ("status","error","spent","received")}
        if loose["status"]!="0x1":continue
        n=int(loose["received"]);exact=one(o,n,ts+60)
        if exact["status"]!="0x1" or exact["spent"]!=o["n"] or int(exact["received"])!=n or n<=0:raise ValueError("terminal payout")
        o["exact"]=exact;o["controls"]=[]
        for lim,due in [(n+1,ts+60),(n,ts-1)]:
            fail=one(o,lim,due)
            if (fail['status'],fail['spent'],fail['received'])!=('0x0','0','0'): raise ValueError("net/expiry")
            o["controls"].append(fail["error"])
    return dict(chain=8453,block=int(latest["number"],16),head=head,ts=ts,fee=fee,source=API,owner=OWNER,wallet=WALLET,
                router=ROUTER,ocean=OCEAN,executor=EXEC,funding=PREPAID,fundingHash="0x"+keccak(hexstr=rpc(url,"eth_getCode",[PREPAID,tag])).hex(),walletHash="0x"+keccak(runtime).hex(),cases=offers)


if __name__=="__main__":
    p=argparse.ArgumentParser();p.add_argument("artifact",type=Path);p.add_argument("output",type=Path);args=p.parse_args();errors=[]
    for endpoint in ["https://mainnet.base.org","https://base.gateway.tenderly.co","https://base.drpc.org"]:
        try:
            result=capture(endpoint,json.loads(args.artifact.read_bytes()))
            args.output.write_text(json.dumps(result,separators=(",",":")),encoding="utf8");break
        except (requests.RequestException,ValueError,KeyError,RuntimeError) as e:errors.append((endpoint,str(e)[:230]))
    else:raise ValueError(errors)
