"""Compare native, guarded, and reserve payouts against an archived public builder frame."""
import argparse
import json
from pathlib import Path
from eth_abi import decode
from eth_utils import keccak
from record_prop_cash_sources import CASES
from record_prop_venue import Capture, ENDPOINTS, MODULE, OWNER, ROUTER, WETH, USDC, USDT, leg, rpc, runtime_with_args, tx

BASE_DIR=Path("tests/data/first_run_dex_2026-11-26")
TOKENS={WETH:10**16,USDC:5_000_000,USDT:5_000_000,"0x5afe3855358e112b5647b952709e6165e1c1eeee":10**16,
        "0xabec6aeab037b77c09a2a99264f9c81a35b02649":10**13}

def capture(endpoint, frame, snapshot, art, guarded_slot=False):
    tag={"blockHash":snapshot["parent"],"requireCanonical":True}
    c=Capture({"endpoint":endpoint,"selector":tag})
    parent=rpc(c,"eth_getBlockByNumber",[hex(snapshot["number"]-1),False])["result"]
    assert rpc(c,"eth_chainId",[])["result"]=="0x1" and parent["hash"]==tag["blockHash"]
    assert int(parent["timestamp"],16)<snapshot["timestamp"]==1606824023+12*snapshot["slot"]
    block={"time":hex(snapshot["timestamp"])}
    deadline=snapshot["timestamp"]
    venue,vault,_=CASES[frame["name"]]
    assert venue==frame["market"] and frame["oracle"] not in (OWNER,MODULE)
    expected=BASE_DIR/(frame["name"]+"_ethereum_wallets.json.gz")
    import gzip
    observed=json.loads(gzip.decompress(expected.read_bytes()))
    roles=[ROUTER,venue,vault]
    code=[rpc(c,"eth_getCode",[a,tag])["result"] for a in roles]
    assert all(v!="0x" for v in code) and all(rpc(c,"eth_getCode",[a,tag])["result"]=="0x" for a in [MODULE,OWNER])
    hashes=[keccak(hexstr=v) for v in code]
    module=runtime_with_args(c,art,["address"]*3+["bytes32"]*3,roles+hashes)
    runtime="0x"+keccak(hexstr=module).hex()
    if not guarded_slot:assert runtime==observed["rows"][0]["runtime"]
    diff={frame["oracle"]:{"stateDiff":frame["storage"]}}
    q=rpc(c,"eth_call",[tx(ROUTER,"fallbackSwapRouter()"),tag])["result"]
    fallback,=decode(["address"],bytes.fromhex(q[2:]))
    def approve(coin,to,size):
        return tx(coin,"approve(address,uint256)",["address","uint256"],[to,size])
    def swap(market,a,b,size):
        return tx(ROUTER,"swapViaVenueV1(address,address,address,uint256,uint256,address,uint256)",
                  ["address","address","address","uint256","uint256","address","uint256"],[market,a,b,size,1,OWNER,deadline])
    result=[]
    for a,b in frame["pairs"]:
        for inp,out in [(a,b),(b,a)]:
            amount=TOKENS[inp]
            prefix=[tx(WETH,"deposit()",value=3*10**17)]
            if inp!=WETH:
                currency=inp if inp in (USDC,USDT) else USDC
                prefix.extend([approve(WETH,ROUTER,10**17),swap(fallback,WETH,currency,10**17)])
                if inp!=currency:
                    prefix.extend([approve(USDC,ROUTER,20_000_000),swap(venue,USDC,inp,20_000_000)])
            forms=[("boundSlot",True)] if guarded_slot else [("original",False),("strict",True),("allowFallback","guarded-fallback")]
            for name,guard in forms:
                order=(keccak(text=frame["builder"]+inp+out),bytes.fromhex(snapshot["parent"][2:]),snapshot["number"],deadline)
                def run(minimum, time, enabled):
                    return leg(c,module,venue,inp,out,amount,time,guard,minimum,prefix,fallback,block,diff if enabled else None,(*order[:3],time) if guarded_slot else None)
                def compact(row):
                    return {k:row[k] for k in ("status","actual","paid","net","gas","approvalGas","error") if k in row}
                info={"form":name,"input":inp,"output":out,"requested":amount}
                # Even when the preparatory purchase is unavailable, preserve the other direction and markets.
                def try_run(minimum,time,enabled):
                    try:
                        return run(minimum,time,enabled)
                    except ValueError as exc:
                        if str(exc)!="isolated personal funding failed":raise
                        return {"status":"funding-unavailable"}
                paid=try_run(1,deadline,True)
                info["withFrame"]=compact(paid)
                info["withoutFrame"]=compact(try_run(1,deadline,False))
                if paid["status"]=="paid":
                    tighter=try_run(paid["net"]+1,deadline,True)
                    expired=try_run(1,deadline-1,True)
                    assert expired["status"]=="reverted"
                    assert tighter["status"]=="reverted" or tighter["status"]=="paid" and tighter["net"]>paid["net"]
                    info.update(tighter=compact(tighter),expired=compact(expired),logs=paid["logs"])
                    if guarded_slot:
                        for label,wrong in [("wrongParent",(order[0],bytes(32),order[2],order[3])),
                                            ("otherHeight",(order[0],order[1],order[2]+1,order[3])),
                                            ("futureSlot",(order[0],order[1],order[2],order[3]+12)),
                                            ("emptyLeg",(bytes(32),*order[1:]))]:
                            verdict=leg(c,module,venue,inp,out,amount,deadline,guard,1,prefix,fallback,block,diff,wrong)
                            assert verdict["status"]=="reverted" and verdict["paid"]==verdict["net"]==0
                            info[label]=compact(verdict)
                result.append(info)
    return {"builder":frame["builder"],"market":venue,"oracle":frame["oracle"],"parent":tag,"timestamp":deadline,"runtime":runtime,"rows":result}

if __name__=="__main__":
    ap=argparse.ArgumentParser();ap.add_argument("artifact",type=Path);ap.add_argument("output",type=Path);ap.add_argument("market")
    ap.add_argument("--slot-guard",action="store_true")
    args=ap.parse_args()
    snapshot=json.loads((BASE_DIR/"prop_ethereum_builder_frames.json").read_bytes())
    art=json.loads(args.artifact.read_bytes())
    out=[]
    frames=[f for f in snapshot["frames"] if f["name"]==args.market]
    assert frames
    for f in frames:
        for endpoint in ENDPOINTS:
            try:
                out.append(capture(endpoint,f,snapshot,art,args.slot_guard));break
            except Exception as exc:print(type(exc).__name__,str(exc)[:180],flush=True)
        else:raise RuntimeError("no endpoint completed builder replay")
    args.output.write_text(json.dumps({"builders":out})+"\n",encoding="utf8")
