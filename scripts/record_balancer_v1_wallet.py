"""Measure protected BPool cash, native/WETH settlement, exact-target refunds and the V5 step."""
import argparse
import gzip
from hashlib import sha256
import json
from pathlib import Path

import requests
from eth_abi import encode
from eth_utils import keccak

from record_balancer_markets import Capture
from record_balancer_v1 import rpc
from record_base_slipstream_executor import ROOT, OWNER, EXEC, RUNTIME, STEP, simulate_block
from record_bitswap_ethereum import balance, tx
from record_ethereum_v2_factories import FUNDING, WETH
from record_safeswap_protected import runtime

ETH = "0x" + "00"*20
MODULE = "0x"+keccak(text="Arbitraj original Balancer V1 isolated receipt").hex()[-40:]
MULTICALL = "0xca11bde05977b3631167028862be2a173976ca11"
ORDER = "(address,address,address,uint256,uint256,uint256,uint256,bool)"
UINT = "uint256"
MAX = 2**256-1


def capture(endpoint,artifact,original):
    at = original["selector"]
    c = Capture(dict(endpoint=endpoint,selector=at))
    code = runtime(c,artifact,[original["factory"],WETH])
    header = rpc(endpoint,"eth_getBlockByNumber",[hex(original["block"]),False])
    if header["hash"] != at["blockHash"] or rpc(endpoint,"eth_chainId",[]) != "0x1":
        raise ValueError("wrong canonical market")
    deadline = int(header["timestamp"],16)+600
    def word(a):
        return "0x"+format(int(a,16),"064x")
    override = {OWNER:{"balance":hex(100*10**18)},MODULE:{"code":code},
                EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
                      "stateDiff":{word("0"):word(OWNER),word("1"):word(WETH)}}}
    if any(rpc(endpoint,"eth_getCode",[a,at]) != "0x" for a in override):
        raise ValueError("cash isolation")
    def bal(t,p):
        return tx(MULTICALL,"getEthBalance(address)",["address"],[p]) if t==ETH else balance(t,p)
    cases = []
    seen = set()
    for row in original["payments"]:
        if "spent" not in row:
            continue
        pool,src,dst,exact = (row[k] for k in ("pool","source","target","exact_input"))
        key = pool,src,dst,exact
        if key in seen:
            continue
        seen.add(key)
        paid,out = int(row["spent"]),int(row["received"])
        cap = paid if exact else paid+max(13,paid//10)
        for mode in (("wallet","native_wallet","v5") if exact else ("wallet","native_wallet")):
            a = ETH if mode=="native_wallet" and src==WETH else src
            b = ETH if mode=="native_wallet" and dst==WETH else dst
            target = EXEC if mode=="v5" else MODULE
            prep = []
            if a == WETH:
                prep.append(tx(WETH,"deposit()",value=cap))
            elif a != ETH:
                prep.append(tx(FUNDING,"swapETHForExactTokens(uint256,address[],address,uint256)",
                    [UINT,"address[]","address",UINT],[cap,[WETH,a],OWNER,deadline],value=10**18))
            if a != ETH:
                prep.append(tx(a,"approve(address,uint256)",["address",UINT],[target,cap]))
            def order(market,maximum,minimum,end):
                if mode == "v5":
                    step = (17,MODULE,a,b,10000,encode(["address"],[market]))
                    return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                        ["address",UINT,"address",UINT,UINT,STEP+"[]"],[a,maximum,b,minimum,end,[step]])
                return tx(MODULE,"execute("+ORDER+")",[ORDER],[(market,a,b,maximum,minimum,MAX,end,not exact)],
                          value=maximum if a==ETH else 0)
            cash = [bal(t,p) for p in (OWNER,MODULE,EXEC) for t in (a,b)]
            allow = tx(src,"allowance(address,address)",["address","address"],[MODULE,pool])
            controls = [order(pool,cap if exact else paid-1,out+1 if exact else out,deadline),
                        order(pool,cap,out,0),order(OWNER,cap,out,deadline)]
            frame = simulate_block(endpoint,at,override,prep+cash+controls+[order(pool,cap,out,deadline)]+cash+[allow],rpc)
            if any(v["status"] != "0x1" for v in frame[:len(prep)]):
                raise ValueError(("external funding unavailable",key,mode))
            first = len(prep)
            before,limits,trade,after = frame[first:first+6],frame[first+6:first+9],frame[first+9],frame[first+10:first+16]
            if any(v["status"]!="0x0" for v in limits) or trade["status"]!="0x1" or any(
                    v["status"]!="0x1" for v in before+after+[frame[-1]]):
                raise ValueError(("cash limits",key,mode,[v.get("error") for v in limits],trade.get("error")))
            pre,post = ([int(v["returnData"],16) for v in group] for group in (before,after))
            if (pre[0]-post[0],post[1]-pre[1]) != (paid,out) or pre[2:]!=post[2:] or int(frame[-1]["returnData"],16):
                raise ValueError(("net wallet or remaining approval",key,mode))
            cases.append(dict(mode=mode,pool=pool,source=src,target=dst,asset_in=a,asset_out=b,
                              exact_input=exact,maximum=str(cap),minimum=str(out),spent=str(paid),received=str(out),
                              gas=int(trade["gasUsed"],16),logs=trade["logs"]))
    if not cases:
        raise ValueError("no paid original sample")
    role = original["factory"]
    creation = bytes.fromhex(artifact["creation"]) + encode(["address","address"],[role,WETH])
    wrapped = bytes.fromhex(rpc(endpoint,"eth_getCode",[WETH,at])[2:])
    if not wrapped:
        raise ValueError("missing WETH runtime")
    module_hash = "0x"+keccak(hexstr=code).hex()
    manifest = dict(kind="balancer_v1",chainId=1,factory=role,wrapped=WETH,
                    factoryHash=original["factory_code"]["keccak256"],wrappedHash="0x"+keccak(wrapped).hex(),
                    sourceBlock=at["blockHash"],creationSha256=sha256(creation).hexdigest(),
                    runtimeSha256=sha256(bytes.fromhex(code[2:])).hexdigest(),runtimeKeccak=module_hash,
                    data="0x"+creation.hex(),solc=artifact["solc"])
    return dict(selector=at,module=MODULE,module_hash=module_hash,cases=cases),manifest


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("compiled",type=Path)
    parser.add_argument("out",type=Path)
    args = parser.parse_args()
    source = ROOT/"tests/data/first_run_dex_2026-11-26/balancer_v1_ethereum.json.gz"
    with gzip.open(source,"rt") as stream:
        original = json.load(stream)
    errors = []
    for url in ("https://eth.drpc.org","https://rpc.mevblocker.io","https://ethereum-rpc.publicnode.com"):
        try:
            record,manifest = capture(url,json.loads(args.compiled.read_bytes()),original)
            break
        except (requests.RequestException,RuntimeError,ValueError,KeyError) as e:
            errors.append((url,str(e)[:550]))
    else:
        raise RuntimeError(errors)
    args.out.with_name("balancer-v1-create.json").write_bytes(json.dumps(manifest,separators=(",",":")).encode())
    args.out.write_bytes(json.dumps(record,separators=(",",":")).encode())
