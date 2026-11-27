"""Exercise each immediate Integral relayer payment through the module at the original canonical block."""
import argparse
import gzip
import json
from pathlib import Path

from eth_abi import decode, encode
from eth_utils import keccak
from record_balancer_markets import Capture
from record_base_slipstream_executor import EXEC, ROOT, RUNTIME, STEP, rpc
from record_bitswap_ethereum import tx
from record_ethereum_v2_factories import WETH
from record_ethereum_x7 import ENDPOINTS
from record_safeswap_protected import runtime

DATA = ROOT / "tests/data/first_run_dex_2026-11-26"
MODULE = "0x" + keccak(text="Integral SIZE archive wallet module")[-20:].hex()
EVENT = "0x" + keccak(text="Filled(address,address,address,address,address,uint256,uint256,uint256,bool)").hex()


def capture(url, artifact):
    def record(name):
        return json.loads(gzip.decompress((DATA / name).read_bytes()))
    inventory = record("integral_size_ethereum.json.gz")
    archives = record("integral_size_ethereum_wallets.json.gz")
    proxy, implementation, factory, queue = [inventory["roles"][k]["address"]
        for k in ("relayerProxy", "relayer", "factory", "delay")]
    history = archives["historical"]
    tag = history["selector"]
    c = Capture(dict(endpoint=url, selector=tag))
    head = rpc(url, "eth_getBlockByHash", [tag["blockHash"],False])
    if head["stateRoot"] != history["state_root"] or rpc(url,"eth_chainId",[]) != "0x1":
        raise ValueError("archive state mismatch")
    for address in (MODULE,EXEC):
        if rpc(url,"eth_getCode",[address,tag]) != "0x":
            raise ValueError("occupied simulation address")
    code = runtime(c, artifact, [proxy,implementation,factory,queue])
    word=lambda n: "0x"+format(n,"064x")
    now = int(head["timestamp"],16) + 12
    rows = []
    for case in history["cases"]:
        payer, x, y = (case[k] for k in ("payer","input","output"))
        amount, net = int(case["paid"]), int(case["net"])
        pool = next(m["pair"] for m in inventory["markets"] if set(m["tokens"])=={x,y})
        exact_out = case["side"]=="buy"
        for mode in (("module-buy",) if exact_out else ("module-sell","v5")):
            target = EXEC if mode=="v5" else MODULE
            override={MODULE:{"code":code}, EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
                      "stateDiff":{word(0):word(int(payer,16)),word(1):word(int(WETH,16))}}}
            def call(to,sig,types=(),args=()):
                req=tx(to,sig,types,args)
                req["from"]=payer
                return req
            def order(cap,minimum,deadline):
                if mode=="v5":
                    return call(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                        ["address","uint256","address","uint256","uint256",STEP+"[]"],
                        [x,cap,y,minimum,deadline,[(29,MODULE,x,y,10000,b"")]])
                method="buy" if exact_out else "swapFrom"
                return call(MODULE,method+"(address,address,uint256,uint256,address,uint256)",
                    ["address","address","uint256","uint256","address","uint256"],[x,y,cap,minimum,payer,deadline])
            approvals=[call(x,"approve(address,uint256)",["address","uint256"],[target,n]) for n in (0,amount+1)]
            checks=[call(token,"balanceOf(address)",["address"],[who]) for who in (payer,MODULE,EXEC) for token in (x,y)]
            checks.append(call(x,"allowance(address,address)",["address","address"],[MODULE,proxy]))
            good=order(amount+int(exact_out),net,now+300)
            strict=order(amount-int(exact_out),net+int(not exact_out),now+300)
            calls=[*approvals,*checks,strict,order(amount,net,now-1),good,*checks]
            block,=rpc(url,"eth_simulateV1",[{"blockStateCalls":[{"calls":calls,"stateOverrides":override,
                        "blockOverrides":{"time":hex(now)}}],"validation":False},tag])
            results=block["calls"]
            if block["parentHash"]!=tag["blockHash"] or len(results)!=len(calls):
                raise ValueError("parent or call count")
            if any(v["status"]!="0x1" for v in results[:9]+results[11:]) or any(
                    v["status"]!="0x0" or v["logs"] for v in results[9:11]):
                raise ValueError((mode,x,[(i,v.get("error")) for i,v in enumerate(results) if v["status"]!="0x1"]))
            before,after=([int(v["returnData"],16) for v in a] for a in (results[2:9],results[-7:]))
            if before[0]-after[0]!=amount or after[1]-before[1]!=net or before[2:]!=after[2:] or after[-1]!=0:
                raise ValueError(("wallet and allowance",mode,before,after))
            fill=results[11]
            matched=[e for e in fill["logs"] if e["address"].lower()==MODULE and e["topics"][:1]==[EVENT]]
            if len(matched)!=1:raise ValueError("missing or repeated settlement")
            event=matched[0]
            sender=EXEC if mode=="v5" else payer
            # The executor may receive the step output itself before transferring it to the wallet.
            epayer,recipient,epool=["0x"+k[-40:] for k in event["topics"][1:]]
            a,b,paid,received,oid,by_target=decode(["address","address","uint256","uint256","uint256","bool"],
                                                   bytes.fromhex(event["data"][2:]))
            if (epayer,epool,a,b,paid,received,oid,by_target)!=(sender,pool,x,y,amount,net,case["order"],exact_out) or recipient not in ({payer,EXEC} if mode=="v5" else {payer}):
                raise ValueError(("attribution",mode,matched))
            rows.append(dict(mode=mode,payer=payer,input=x,output=y,pair=pool,spent=str(amount),received=str(net),
                             order=oid,gas=int(fill["gasUsed"],16),calls=calls,results=results))
    return dict(chain=1,selector=tag,root=head["stateRoot"],roles=[proxy,implementation,factory,queue],
                module=MODULE,runtime_hash="0x"+keccak(hexstr=code).hex(),cases=rows)


if __name__=="__main__":
    cli=argparse.ArgumentParser(description=__doc__)
    cli.add_argument("artifact",type=Path)
    cli.add_argument("output",type=Path)
    args=cli.parse_args()
    for endpoint in ENDPOINTS:
        try:
            result=capture(endpoint,json.loads(args.artifact.read_bytes()))
            break
        except (OSError,RuntimeError) as e:
            print(str(e)[:260],flush=True)
    else:raise RuntimeError("historical endpoint unavailable")
    args.output.write_text(json.dumps(result,separators=(",",":")),encoding="utf8")
