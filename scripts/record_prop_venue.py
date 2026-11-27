"""Capture public PropAMMRouter quotes and wallet deltas; the only funded account belongs to this simulation."""
import argparse
import json
from pathlib import Path
from eth_abi import decode, encode
from eth_utils import keccak
from record_balancer_markets import Capture
from record_ethereum_x7 import ENDPOINTS
from record_safeswap_protected import runtime_with_args

ROUTER="0x4ddf368080cd7946db5b459ad591c350158175e1"
WETH="0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"
USDC="0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"
USDT="0xdac17f958d2ee523a2206206994597c13d831ec7"
GOLIATH="0x4367985a63063fc970b009c6e599e9225181f551"
TEMPEST="0x00000003f1ec2379e79f58e12ec6c4f51ee92149"
OWNER="0x"+keccak(text="Arbitraj PropAMM public wallet regression")[-20:].hex()
MODULE="0x"+keccak(text="Arbitraj PropAMM strict venue module")[-20:].hex()
SWAPPED="0x"+keccak(text="Swapped(address,address,address,uint256,uint256,address,address)").hex()
HEADS=[(26480795,GOLIATH,USDC),(26487844,TEMPEST,USDT),(26494988,GOLIATH,USDC),(26494988,TEMPEST,USDT)]

ANCHORS={26480795: '0xa76d204538aac92e1c8c12654bfcec22895375a8cf58be25a4bbed0868728a60', 26487844: '0x6a562bc5d13551d7272f0395a9a89298c104bdd252143d9283e2283495b7c03a', 26494988: '0x2372196a2961b307333be48d3dad74a2e723fd459c2cb01e225a0a97f50e7a7a'}

def rpc(c,method,params):
    if method not in {"eth_chainId","eth_getBlockByNumber","eth_getCode","eth_call","eth_simulateV1"}:
        raise ValueError(method)
    res=c.session.post(c.endpoint,json={"jsonrpc":"2.0","id":1,"method":method,"params":params},timeout=60)
    res.raise_for_status()
    return res.json()

def tx(to,sig,types=(),values=(),value=0):
    return {"from":OWNER,"to":to,"value":hex(value),"gas":hex(3_000_000),"data":"0x"+(keccak(text=sig)[:4]+encode(types,values)).hex()}

def leg(c,code,venue,inp,out,n,t,strict,floor,prefix,fallback):
    dst=MODULE if strict else ROUTER
    apps=[tx(inp,"approve(address,uint256)",["address","uint256"],[dst,n])]
    bal=lambda token:tx(token,"balanceOf(address)",["address"],[OWNER])
    if strict=="guarded-fallback":
        order=tx(MODULE,"executeWithFallback(address,address,address,uint256,uint256,uint256,address)",
                 ["address","address","address","uint256","uint256","uint256","address"],[venue,inp,out,n,floor,t,fallback])
    elif strict:
        order=tx(MODULE,"execute(address,address,address,uint256,uint256,uint256)",
                 ["address","address","address","uint256","uint256","uint256"],[venue,inp,out,n,floor,t])
    else:
        order=tx(ROUTER,"swapViaVenueV1(address,address,address,uint256,uint256,address,uint256)",
                 ["address","address","address","uint256","uint256","address","uint256"],[venue,inp,out,n,floor,OWNER,t])
    calls=prefix+apps+[bal(inp),bal(out),order,bal(inp),bal(out)]
    info=rpc(c,"eth_simulateV1",[{"blockStateCalls":[{"stateOverrides":{OWNER:{"balance":hex(2*10**18)},MODULE:{"code":code}},"calls":calls}]},c.selector])
    raw=info["result"][0]
    assert raw["parentHash"]==c.selector["blockHash"]
    results=raw["calls"];before=results[-5:-3];swap=results[-3];after=results[-2:]
    if any(x["status"]!="0x1" for x in results[:-3]+after):raise ValueError("isolated personal funding failed")
    val=lambda x:int(x["returnData"],16)
    paid=val(before[0])-val(after[0]);net=val(after[1])-val(before[1])
    if swap["status"]!="0x1":
        if paid or net:raise ValueError("reverted payment")
        return {"status":"reverted","error":swap.get("error"),"paid":0,"net":0}
    events=[e for e in swap["logs"] if e["address"].lower()==ROUTER and e["topics"][0]==SWAPPED]
    if len(events)!=1:raise ValueError("ambiguous source")
    sold,received,to,actual=decode(["uint256","uint256","address","address"],bytes.fromhex(events[0]["data"][2:]))
    expected=[MODULE if strict else OWNER,inp,out]
    if [str(x)[-40:].lower() for x in events[0]["topics"][1:]]!=[x[2:] for x in expected]:raise ValueError("wrong indexed wallet or currencies")
    if (sold,received,to)!=(n,net,OWNER) or paid!=n or net<floor or (strict is True and actual!=venue) or actual not in (venue,fallback):raise ValueError("wallet settlement mismatch")
    return {"status":"paid","paid":paid,"net":net,"actual":actual,"gas":int(swap["gasUsed"],16),"logs":swap["logs"],"order":order,"approvalGas":int(results[len(prefix)]["gasUsed"],16)}

def capture(endpoint,art,kipseli=False,market_name=None):
    result=[]
    market='0x71e790dd841c8a9061487cb3e78c288e75ce0b3d'
    heads=HEADS
    vaults={GOLIATH:"0x93bf01ce743579f928d30d05695824886bd5f37a",
            TEMPEST:"0xc9d748e601d9984a43da0b80e5b91dc28d31d9fb",
            market:"0xbeeb9eee061925cc6d2122f05a4e6536f0feb000"}
    if market_name:
        from record_prop_cash_sources import CASES
        venue,vault,heights=CASES[market_name]
        vaults[venue]=vault
        heads=[(h,venue,USDC) for h in heights]
        ANCHORS.update(heights)
    if kipseli:
        heads=[(26487846,market,USDC),(26494988,market,USDC)]
        ANCHORS[26487846]='0x3238e9c17c5ae484f27f9e2cab7a749b243cce3cd250db16d0909d026e52ad47'
    for height,venue,stable in heads:
        c=Capture({"endpoint":endpoint,"selector":{"blockHash":ANCHORS[height],"requireCanonical":True}})
        header=rpc(c,"eth_getBlockByNumber",[hex(height),False])["result"]
        if header["hash"]!=ANCHORS[height] or rpc(c,"eth_chainId",[])["result"]!="0x1":raise ValueError("unexpected chain or head")
        for a in [OWNER,MODULE]:
            if rpc(c,"eth_getCode",[a,c.selector])["result"]!="0x":raise ValueError("test address in use")
        vault=vaults[venue]
        roles=[ROUTER,venue,vault]
        hashes=[keccak(bytes.fromhex(rpc(c,"eth_getCode",[role,c.selector])["result"][2:])) for role in roles]
        code=runtime_with_args(c,art,["address"]*3+["bytes32"]*3,roles+hashes)
        deadline=int(header["timestamp"],16)+180
        rhs=tx(ROUTER,"fallbackSwapRouter()")
        fallback=decode(["address"],bytes.fromhex(rpc(c,"eth_call",[rhs,c.selector])["result"][2:]))[0]
        for inp,out,n in [(WETH,stable,10**16),(WETH,stable,2*10**16),(stable,WETH,5_000_000),(stable,WETH,10_000_000)]:
            prefix=[tx(WETH,"deposit()",value=3*10**17)]
            if inp==stable:
                # Buy the initial stablecoin using only this wallet's WETH at the published Uni fallback.
                prefix += [tx(WETH,"approve(address,uint256)",["address","uint256"],[ROUTER,10**17]),
                           tx(ROUTER,"swapViaVenueV1(address,address,address,uint256,uint256,address,uint256)",
                            ["address","address","address","uint256","uint256","address","uint256"],[fallback,WETH,stable,10**17,1,OWNER,deadline])]
            quote=tx(ROUTER,"quoteVenueV1(address,address,address,uint256)",["address","address","address","uint256"],[venue,inp,out,n])
            q=rpc(c,"eth_call",[quote,c.selector]);raw=q.get("result")
            if raw:
                point,quoted=decode(["uint256","address"],bytes.fromhex(raw[2:]))
                if quoted!=venue:raise ValueError("quote from another venue")
            for strict in [False,True,"guarded-fallback"]:
                observed=leg(c,code,venue,inp,out,n,deadline,strict,1,prefix,fallback)
                if observed["status"]=="paid":
                    tighter=leg(c,code,venue,inp,out,n,deadline,strict,observed["net"]+1,prefix,fallback)
                    if tighter["status"]=="paid" and tighter["net"]<observed["net"]+1:raise ValueError("signed floor")
                    expired=leg(c,code,venue,inp,out,n,int(header["timestamp"],16),strict,1,prefix,fallback)
                    if expired["status"]!="reverted":raise ValueError("expired order accepted")
                    observed.update(tighter=tighter,expired=expired)
                result.append({"height":height,"selector":c.selector,"venue":venue,"fallback":fallback,"input":inp,"output":out,
                               "amount":n,"strict":strict is True,"form":("guarded-fallback" if strict=="guarded-fallback" else ("strict" if strict else "router")),"quote":q,"runtime":"0x"+keccak(bytes.fromhex(code[2:])).hex(),**observed})
    return {"owner":OWNER,"module":MODULE,"rows":result}

if __name__=="__main__":
    ap=argparse.ArgumentParser();ap.add_argument("artifact",type=Path);ap.add_argument("output",type=Path);ap.add_argument("--kipseli",action="store_true");ap.add_argument("--source",choices=["kaliber","taurus"]);args=ap.parse_args()
    for endpoint in ENDPOINTS:
        try:
            data=capture(endpoint,json.loads(args.artifact.read_text(encoding="utf8")),args.kipseli,args.source)
            args.output.write_text(json.dumps(data)+"\n",encoding="utf8");break
        except Exception as exc:
            print(type(exc).__name__,str(exc)[:240],flush=True)
    else:raise RuntimeError("no endpoint completed the public wallet archive")
