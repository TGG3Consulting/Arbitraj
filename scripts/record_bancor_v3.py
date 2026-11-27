"""Ethereum Bancor V3 directory, original quote formulas, signed limits and isolated cash settlement."""
import argparse
import json
from pathlib import Path

import requests
from eth_abi import decode
from eth_utils import keccak

from record_balancer_markets import Capture, posts
from record_base_slipstream_executor import OWNER, ROOT, simulate_block
from record_bitswap_ethereum import balance, tx
from record_ethereum_v2_factories import FUNDING, STABLES, WETH

REV = "97d7905b8548470e79d40ce02e09b006f555aa11"
MANIFEST = f"https://raw.githubusercontent.com/bancorprotocol/contracts-v3/{REV}/deployments/mainnet/"
NATIVE = "0x" + "ee" * 20
MULTI = "0xca11bde05977b3631167028862be2a173976ca11"
SLOT = "0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc"
MILLION = 1_000_000


def leg(source, target, fee, amount, exact_input):
    if exact_input:
        gross = target * amount // (source + amount)
        return gross - gross * fee // MILLION
    gross = amount + amount * fee // (MILLION - fee)
    return source * gross // (target - gross) if gross < target else None


def capture(endpoint):
    snapshot = json.loads((ROOT / "tests/data/first_run_dex_2026-11-26/ethereum.json").read_bytes())
    c = Capture(dict(snapshot, endpoint=endpoint))
    at = c.selector

    def rpc(method, params):
        if method not in ("eth_chainId","eth_getBlockByHash","eth_getCode","eth_getStorageAt","eth_simulateV1"):
            raise ValueError("unsupported read")
        response = c.session.post(endpoint, json=dict(jsonrpc="2.0", id=1, method=method, params=params), timeout=45)
        response.raise_for_status()
        msg = response.json()
        if "error" in msg:
            raise RuntimeError(str(msg["error"])[:220])
        return msg["result"]

    def role(addr):
        raw = bytes.fromhex(rpc("eth_getCode",[addr,at])[2:])
        if not raw:
            raise ValueError("empty role " + addr)
        return dict(address=addr,keccak256="0x"+keccak(raw).hex(),bytes=len(raw))

    header = rpc("eth_getBlockByHash", [at["blockHash"],False])
    if rpc("eth_chainId",[]) != "0x1" or header["hash"] != at["blockHash"]:
        raise ValueError("incorrect chain")
    roles = {}
    for name in ("BancorNetwork","BancorNetworkInfo","MasterVault","BNTPool","NetworkSettings"):
        ref = requests.get(MANIFEST + name + ".json", timeout=20)
        ref.raise_for_status()
        manifest = ref.json()
        address = manifest["address"].lower()
        impl = "0x" + rpc("eth_getStorageAt", [address,SLOT,at])[-40:]
        roles[name] = dict(**role(address),implementation=role(impl),manifest_implementation=manifest["implementation"].lower())
    def view(to, signature, returns, types=(), args=()):
        return c.one(to, signature, types, args, [returns])[0]
    network, info = [roles[n]["address"] for n in ("BancorNetwork","BancorNetworkInfo")]
    if view(info,"network()","address") != network or view(info,"masterVault()","address") != roles["MasterVault"]["address"]:
        raise ValueError("unexpected network or vault")
    paused = view(network,"paused()","bool")
    bnt = view(info,"bnt()","address")
    directory = view(network,"liquidityPools()","address[]")
    if len(set(directory)) != len(directory) or bnt in directory:
        raise ValueError("ambiguous pools")
    # Allow all pools in the directory, including paused markets and native ETH.
    methods = [(network,"collectionByPool(address)","address"),(info,"tradingEnabled(address)","bool"),
               (info,"tradingFeePPM(address)","uint32"),(info,"tradingLiquidity(address)","(uint128,uint128)")]
    raw = posts(c,[(contract,sig,["address"],[token]) for token in directory for contract,sig,_ in methods])
    pools = {}
    for i, token in enumerate(directory):
        collection, enabled, fee, (bnt_depth, base_depth) = [
            decode([ret],raw[len(methods)*i+j])[0] for j,(_,_,ret) in enumerate(methods)]
        pools[token] = dict(collection=collection,enabled=enabled,fee=fee,bnt=str(bnt_depth),base=str(base_depth))
    collections = {a:role(a) for a in sorted({v["collection"] for v in pools.values()})}

    def model(source, target, amount, exact_input):
        steps = []
        if source != bnt:
            m = pools[source]
            steps.append((int(m["base"]),int(m["bnt"]),m["fee"]))
        if target != bnt:
            m = pools[target]
            steps.append((int(m["bnt"]),int(m["base"]),m["fee"]))
        for a,b,fee in steps if exact_input else reversed(steps):
            amount = leg(a,b,fee,amount,exact_input)
            if not amount:
                return amount
        return amount

    def local_balance(token):
        return tx(MULTI,"getEthBalance(address)",["address"],[OWNER]) if token==NATIVE else balance(token,OWNER)
    deadline = int(header["timestamp"],16)+300
    srcs = [t for t in [NATIVE,STABLES[0],STABLES[1],WETH] if pools.get(t,{}).get("enabled")]
    if not srcs or paused:
        return dict(block=snapshot["block"],selector=at,source_commit=REV,roles=roles,pools=pools,collections=collections,paused=paused,bnt=bnt,cases=[])
    candidates = [(t,bnt) for t in srcs]+[(bnt,t) for t in srcs]
    candidates.extend((a,b) for a in srcs[:2] for b in srcs[:2] if a!=b)
    cases = []
    for source,target in candidates:
        ds = 18 if source==NATIVE else view(source,"decimals()","uint8")
        dt = 18 if target==NATIVE else view(target,"decimals()","uint8")
        for exact_input in (True,False):
            amount = 10**(ds if exact_input else dt)//1000
            predicted = model(source,target,amount,exact_input)
            if not predicted:
                continue
            signature = ("tradeOutputBySourceAmount" if exact_input else "tradeInputByTargetAmount")+"(address,address,uint256)"
            price = view(info,signature,"uint256",["address","address","uint256"],[source,target,amount])
            if price != predicted:
                raise ValueError(("formula discrepancy", source,target,amount,exact_input,price,predicted))
            spent, received = (amount,price) if exact_input else (price,amount)
            args = ["address","address","uint256","uint256","uint256","address"]
            def trade(limit,expiry):
                fn = "tradeBySourceAmount" if exact_input else "tradeByTargetAmount"
                return tx(network,fn+"(address,address,uint256,uint256,uint256,address)",args,
                          [source,target,amount,limit,expiry,OWNER],value=spent if source==NATIVE else 0)
            prep = []
            if source == WETH:
                prep.append(tx(WETH,"deposit()",value=spent))
            elif source != NATIVE:
                prep.append(tx(FUNDING,"swapETHForExactTokens(uint256,address[],address,uint256)",
                               ["uint256","address[]","address","uint256"],[spent,[WETH,source],OWNER,deadline],value=10**18))
            if source != NATIVE:
                prep.append(tx(source,"approve(address,uint256)",["address","uint256"],[network,spent]))
            cash = [local_balance(source),local_balance(target)]
            limit = received if exact_input else spent
            calls = prep+cash+[trade(limit+(1 if exact_input else -1),deadline),trade(limit,0),trade(limit,deadline)]+cash
            frame = simulate_block(endpoint,at,{OWNER:{"balance":hex(100*10**18)}},calls,lambda _,method,params:rpc(method,params))
            rec = dict(source=source,target=target,amount=str(amount),exact_input=exact_input,price=str(price))
            if any(c["status"] != "0x1" for c in frame[:len(prep)]):
                rec["funding_reverted"]=True
            else:
                before,reverts,paid,after=frame[-7:-5],frame[-5:-3],frame[-3],frame[-2:]
                if any(c["status"] != "0x0" for c in reverts) or any(c["status"] != "0x1" for c in before+after):
                    raise ValueError("boundary control failed")
                if paid["status"] != "0x1":
                    rec["revert"] = paid.get("error",{}).get("message","execution reverted")
                else:
                    left,right = [int(r["returnData"],16) for r in before]
                    x,y = [int(r["returnData"],16) for r in after]
                    if (left-x,y-right)!=(spent,received):
                        raise ValueError(("actual wallet differs",source,target,spent,received,left-x,y-right))
                    rec.update(spent=str(spent),received=str(received),gas=int(paid["gasUsed"],16),logs=paid["logs"])
            cases.append(rec)
    return dict(block=snapshot["block"],selector=at,source_commit=REV,roles=roles,pools=pools,collections=collections,paused=paused,bnt=bnt,cases=cases)


if __name__=="__main__":
    cli=argparse.ArgumentParser(description=__doc__)
    cli.add_argument("output",type=Path)
    args=cli.parse_args()
    failures=[]
    for endpoint in ["https://eth.drpc.org","https://rpc.mevblocker.io","https://ethereum-rpc.publicnode.com"]:
        try:
            data=capture(endpoint)
            break
        except (requests.RequestException,RuntimeError,ValueError,KeyError) as exc:
            failures.append((endpoint,str(exc)[:220]))
    else:
        raise RuntimeError(failures)
    args.output.write_text(json.dumps(data,separators=(",",":")),encoding="utf8")
