"""Exercise original factory routes through the protected wallet and the unmodified V5 executor."""
import argparse
import gzip
import json
from hashlib import sha256
from pathlib import Path

import requests
from eth_abi import encode
from eth_utils import keccak

from record_balancer_markets import Capture
from record_balancer_v1 import rpc
from record_base_slipstream_executor import EXEC, OWNER, ROOT, RUNTIME, STEP, simulate_block
from record_bitswap_ethereum import balance, tx
from record_ethereum_v2_factories import WETH
from record_safeswap_protected import runtime
from record_yield_basis_cash import UINT, approve, funding_calls, why

MODULE = "0x"+keccak(text="Arbitraj archive YieldBasis wallet").hex()[-40:]
ETH = "0x"+"00"*20
MULTICALL = "0xca11bde05977b3631167028862be2a173976ca11"
ORDER = "(bytes32,address,address,uint256,uint256,uint256)"
ROLES = ("cryptopool","amm","virtual_pool")


def capture(endpoint,artifact,book):
    at = book["selector"]
    header = rpc(endpoint,"eth_getBlockByNumber",[hex(book["block"]),False])
    if rpc(endpoint,"eth_chainId",[]) != "0x1" or header["hash"] != at["blockHash"]:
        raise ValueError("wrong venue snapshot")
    ts = int(header["timestamp"],16)
    factory = book["factory"]
    code = runtime(Capture(dict(endpoint=endpoint,selector=at)),artifact,[factory,WETH])
    def word(n):
        return "0x"+format(n,"064x")
    overrides = {OWNER:{"balance":hex(10**18)},MODULE:{"code":code},
                 EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
                       "stateDiff":{word(0):word(int(OWNER,16)),word(1):word(int(WETH,16))}}}
    if any(rpc(endpoint,"eth_getCode",[a,at]) != "0x" for a in overrides):
        raise ValueError("unexpected code in test wallet")
    def cash(asset,owner):
        return tx(MULTICALL,"getEthBalance(address)",["address"],[owner]) if asset==ETH else balance(asset,owner)
    results = []
    samples = set()
    for venue,entry in book["venues"].items():
        index = entry["markets"][0]
        market = book["markets"][index]
        packed = (index<<168) | (ROLES.index(entry["kind"])<<160) | int(venue,16)
        for case in entry["cases"]:
            if case["stage"] not in ("pool rejected","paid"):
                raise ValueError(("unresolved funding",venue,case["stage"]))
            src,dst = case["source"],case["target"]
            amount = int(case["amount"])
            success = case["stage"]=="paid"
            output = int(case["received"]) if success else 1
            for mode in (["wallet","v5","native"] if WETH in (src,dst) else ["wallet","v5"]):
                a = ETH if src==WETH and mode=="native" else src
                b = ETH if dst==WETH and mode=="native" else dst
                prep = funding_calls(market,book["stable"],src,case["side"],case["funding"],ts)
                if a==ETH:
                    # Preserve state-changing preparations even when the source token is WETH.
                    prep.append(tx(WETH,"withdraw(uint256)",[UINT],[amount]))
                else:
                    prep.append(approve(a,EXEC if mode=="v5" else MODULE,amount))
                def order(key,minimum,end):
                    route = key.to_bytes(32,"big")
                    if mode=="v5":
                        step = (17,MODULE,a,b,10000,route)
                        return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                                  ["address",UINT,"address",UINT,UINT,STEP+"[]"],[a,amount,b,minimum,end,[step]])
                    return tx(MODULE,"execute("+ORDER+")",[ORDER],[(route,a,b,amount,minimum,end)],value=amount if a==ETH else 0)
                balances = [cash(token,owner) for owner in (OWNER,MODULE,EXEC) for token in (a,b)]
                allowance = tx(src,"allowance(address,address)",["address","address"],[MODULE,venue])
                controls = [order(packed,output+1,ts+600),order(packed,1,0),order(packed^1,1,ts+600)] if success else []
                frame = simulate_block(endpoint,at,overrides,prep+balances+controls+[order(packed,output,ts+600)]+balances+[allowance],rpc)
                first = len(prep)+len(balances)
                if any(v["status"]!="0x1" for v in frame[:first]) or any(v["status"]!="0x1" for v in frame[-7:]):
                    raise ValueError(("funding/measurement",venue,mode))
                if any(v["status"]!="0x0" for v in frame[first:first+len(controls)]):
                    raise ValueError(("invalid order accepted",venue,mode))
                trade = frame[-8]
                if (trade["status"]=="0x1")!=success:
                    raise ValueError(("different admission",entry["kind"],venue,mode,case["side"],amount,why(trade)))
                before,after = ([int(v["returnData"],16) for v in rows] for rows in (frame[first-6:first],frame[-7:-1]))
                actual = (before[0]-after[0],after[1]-before[1])
                if actual!=((amount,output) if success else (0,0)) or before[2:]!=after[2:] or int(frame[-1]["returnData"],16):
                    raise ValueError(("wallet delta",venue,mode,actual))
                if success and not any(log["address"].lower()==MODULE and log["topics"][:1]==
                        ["0x"+keccak(text="Paid(address,address,address,address,address,uint256,uint256)").hex()] for log in trade["logs"]):
                    raise ValueError(("missing measured receipt",venue,mode))
                results.append(dict(index=index,kind=entry["kind"],venue=venue,mode=mode,side=case["side"],source=a,target=b,
                                    amount=str(amount),spent=str(actual[0]),received=str(actual[1]),stage=case["stage"],
                                    reason=None if success else why(trade),gas=int(trade["gasUsed"],16)))
                key=(entry["kind"],mode,case["side"])
                if success and key not in samples:
                    results[-1]["logs"]=trade["logs"]
                    samples.add(key)
    creation = bytes.fromhex(artifact["creation"])+encode(["address","address"],[factory,WETH])
    manifest=dict(kind="yield_basis",chainId=1,factory=factory,wrapped=WETH,factoryHash=book["factory_code"]["keccak256"],
                  wrappedHash="0x"+keccak(hexstr=rpc(endpoint,"eth_getCode",[WETH,at])).hex(),sourceBlock=at["blockHash"],
                  creationSha256=sha256(creation).hexdigest(),runtimeSha256=sha256(bytes.fromhex(code[2:])).hexdigest(),
                  runtimeKeccak="0x"+keccak(hexstr=code).hex(),data="0x"+creation.hex(),solc=artifact["solc"])
    return dict(block=book["block"],selector=at,module=MODULE,cases=results),manifest


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("compiled",type=Path)
    parser.add_argument("out",type=Path)
    args=parser.parse_args()
    fixture=ROOT/"tests/data/first_run_dex_2026-11-26/yield_basis_ethereum.json.gz"
    original=json.loads(gzip.decompress(fixture.read_bytes()))
    errors=[]
    for url in ("https://eth.drpc.org","https://rpc.mevblocker.io","https://ethereum-rpc.publicnode.com"):
        try:
            data,manifest=capture(url,json.loads(args.compiled.read_bytes()),original)
            break
        except (requests.RequestException,RuntimeError,KeyError,ValueError) as e:
            errors.append((url,str(e)[:500]))
    else:
        raise RuntimeError(errors)
    args.out.write_bytes(json.dumps(data,separators=(",",":")).encode())
    args.out.with_name("yield-basis-create.json").write_bytes(json.dumps(manifest,separators=(",",":")).encode())
