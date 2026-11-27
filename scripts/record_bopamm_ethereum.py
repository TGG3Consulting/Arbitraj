"""Compare public bopAMM books and funded LambdaClass payments before/after an actually mined price update."""
import argparse
import json
from pathlib import Path

from eth_abi import decode
from eth_account import Account
from eth_account.typed_transactions import TypedTransaction
from eth_utils import keccak
from hexbytes import HexBytes

from fork_fee_rpc import Reader
from record_base_slipstream_executor import OWNER, simulate_block
from record_bitswap_ethereum import tx, balance
from record_concave_ethereum import approval
from record_ethereum_v2_factories import WETH, FUNDING
from record_llama_base_cl import CanonicalCalls

DATA=Path(__file__).resolve().parents[1]/"tests/data/first_run_dex_2026-11-26"
USDC="0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"
USDT="0xdac17f958d2ee523a2206206994597c13d831ec7"
VENUES=["0x160141a205f5ddcf096ba3f48b7ed21eb52c62ea","0xdb13ad0fcd134e9c48f2fdaea8f6751a0f5349ca",
        "0xb09aaa5614916d7aeb59c295c52c92ca82addd76","0xb09aaa8933626d7e4c48d65dad2d77021cfbca9a"]
AMM=VENUES[-1]; ROUTER="0x4ddf368080cd7946db5b459ad591c350158175e1"
REGISTRY="0xda7afeed021eafc1c1af9c362de477dad0396b81"
PAMM="0xb09999a44e3240193193be9dd99cc73c8cc945a6"
MARKET_EVENT="0x"+keccak(text="Swapped(address,address,address,uint256,uint256,address)").hex()
LEGACY_EVENT="0x"+keccak(text="Swap(address,address,address,uint256,uint256)").hex()
FILL="0x"+keccak(text="MakerFill(uint8,address,address,uint256,uint256)").hex()
ROUTER_EVENT="0x"+keccak(text="Swapped(address,address,address,uint256,uint256,address,address)").hex()
FRAME_TX=["0xe20da07ba9b9ade4983ba6c38fd920746deef60efc022368929f9982be11c68c",
          "0x4a4dffc1ae5b7668a27f9995a8a5cf194925af99a743c713b279031cf2a796b4"]
def raw(hexval):return bytes.fromhex(hexval.removeprefix("0x"))
def selector(h):return {"blockHash":h["hash"],"requireCanonical":True}
def signed_update(reader,digest):
    t,receipt=reader.many([(m,[digest]) for m in ("eth_getTransactionByHash","eth_getTransactionReceipt")])
    h=reader.one("eth_getBlockByHash",[t["blockHash"],False])
    parent=reader.one("eth_getBlockByHash",[h["parentHash"],False])
    at=selector(parent)
    assert h["transactions"][int(t["transactionIndex"],16)]==digest and t["blockHash"]==receipt["blockHash"]==h["hash"]
    assert receipt["status"]=="0x1" and int(t["chainId"],16)==1 and t["type"]=="0x2" and t["to"]==REGISTRY
    assert PAMM[2:] in t["input"] and OWNER.lower()!=t["from"]
    v={k:int(t[k],16) for k in ("nonce","chainId","type","gas","value","maxPriorityFeePerGas","maxFeePerGas","r","s","v")}
    v.update(data=HexBytes(t["input"]),to=HexBytes(t["to"]),accessList=t["accessList"])
    binary=TypedTransaction.from_dict(v).encode()
    assert "0x"+keccak(binary).hex()==digest and Account.recover_transaction(binary).lower()==t["from"]
    nonce,payer_code=reader.many([("eth_getTransactionCount",[t["from"],at]),("eth_getCode",[OWNER,at])])
    assert int(nonce,16)==v["nonce"] and payer_code=="0x"
    original={k:t[k] for k in ("from","to","value","nonce","gas","maxFeePerGas","maxPriorityFeePerGas")}
    original["data"]=t["input"]
    return h,at,original

def directory(reader,snap):
    cap=CanonicalCalls(reader);roles={}
    for address,code in zip(VENUES,reader.many(("eth_getCode",[v,reader.tag]) for v in VENUES),strict=True):
        assert code!="0x";roles[address]={"runtime":"0x"+keccak(hexstr=code).hex(),"bytes":len(raw(code))}
    for address in VENUES[-2:]:
        p,=cap.one(address,"pricing()",returns=["address"])
        pairs,=cap.one(address,"getPairs()",returns=["(address,address)[]"])
        paused,=cap.one(address,"paused()",returns=["bool"])
        roles[address].update(pricing=p,paused=paused,pairs=pairs,active=[
            cap.one(address,"isActive(address,address)",["address","address"],list(pair),["bool"])[0] for pair in pairs])
    start=snap["block"]-7200; logs=[]
    for lo in range(start,snap["block"]+1,1800):
        logs+=reader.one("eth_getLogs",[{"address":VENUES,"topics":[[MARKET_EVENT,LEGACY_EVENT]],"fromBlock":hex(lo),"toBlock":hex(min(lo+1799,snap["block"]))}])
    assert all(not x.get("removed") for x in logs) and len({(v["blockHash"],v["logIndex"]) for v in logs})==len(logs)
    pricing_roles={}
    for address,code in zip((PAMM,REGISTRY),reader.many(("eth_getCode",[v,reader.tag]) for v in (PAMM,REGISTRY)),strict=True):
        assert code!="0x"
        pricing_roles[address]="0x"+keccak(hexstr=code).hex()
    return dict(selector=reader.tag,height=snap["block"],state_root=reader.header["stateRoot"],roles=roles,pricing_roles=pricing_roles,
                start=start,logs=logs,source="DefiLlama/dimension-adapters@be9967c3 dexs/bebop/index.ts")

def capture_frame(reader,digest):
    h,at,update=signed_update(reader,digest)
    deadline=int(h["timestamp"],16)+60
    def simulate(calls,updated):
        def request(_url,m,p):
            p[0]["blockStateCalls"][0]["blockOverrides"]={"time":h["timestamp"],"feeRecipient":h["miner"],"gasLimit":h["gasLimit"]}
            return reader.one(m,p)
        result=simulate_block(None,at,{OWNER:{"balance":hex(10**18)}},([update] if updated else [])+calls,read_rpc=request)
        if updated:assert result.pop(0)["status"]=="0x1"
        return result
    cases=[]
    for a,b,n in [(WETH,USDC,10**15),(WETH,USDC,10**16),(USDC,WETH,10**6),(USDC,WETH,10**7),
                  (WETH,USDT,10**15),(USDT,WETH,10**6)]:
        prefix=([tx(WETH,"deposit()",value=n)] if a==WETH else [tx(FUNDING,"swapETHForExactTokens(uint256,address[],address,uint256)",
                ["uint256","address[]","address","uint256"],[n,[WETH,a],OWNER,deadline],value=10**17)])
        reads=[balance(t,who) for who in [OWNER,AMM] for t in [a,b]]
        q=tx(AMM,"quote(address,address,uint256)",["address","address","uint256"],[a,b,n])
        def pay(floor,end):return tx(ROUTER,"swapViaVenueV1(address,address,address,uint256,uint256,address,uint256)",
            ["address","address","address","uint256","uint256","address","uint256"],[AMM,a,b,n,floor,OWNER,end])
        for updated in [False,True]:
            base=prefix+[approval(a,ROUTER,n),*reads,q];i=len(base)
            def leg(floor,end):
                action=pay(floor,end);r=simulate(base+[action,*reads],updated)
                assert all(v["status"]=="0x1" for v in r[:i-1]+r[i+1:])
                before=[int(v["returnData"],16) for v in r[i-5:i-1]];after=[int(v["returnData"],16) for v in r[i+1:]]
                x=r[i];res={"status":x["status"],"quote":r[i-1].get("returnData") if r[i-1]["status"]=="0x1" else None}
                if x["status"]!="0x1":
                    assert before==after
                    return dict(**res,error=x.get("error"))
                spent=before[0]-after[0];net=after[1]-before[1]
                amount,venue=decode(["uint256","address"],raw(x["returnData"]))
                assert spent==n and net==amount and net>=floor and before[2:]==after[2:]
                own=[l for l in x["logs"] if l["address"]==AMM and l["topics"][0]==MARKET_EVENT]
                assert len(own)==int(venue==AMM)
                if own:assert decode(["uint256","uint256","address"],raw(own[0]["data"]))==(n,net,OWNER.lower())
                return dict(**res,venue=venue,spent=str(spent),net=str(net),gas=int(x["gasUsed"],16),tx=action,logs=x["logs"])
            seed=leg(1,deadline)
            if seed["status"]=="0x1":
                net=int(seed["net"]);exact=leg(net,deadline);high=leg(net+1,deadline);expired=leg(1,int(h["timestamp"],16)-1)
                assert exact["status"]=="0x1" and exact["net"]==seed["net"] and exact["venue"]==seed["venue"] and expired["status"]=="0x0"
                assert high["status"]=="0x0" or int(high["net"])>=net+1
                seed.update(boundary=high,expired=expired)
            cases.append(dict(updated=updated,input=a,output=b,amount=str(n),result=seed))
    return dict(height=int(h["number"],16),selector=at,hash=h["hash"],timestamp=int(h["timestamp"],16),builder=h["miner"],update=digest,cases=cases)

if __name__=="__main__":
    ap=argparse.ArgumentParser();ap.add_argument("output",type=Path);args=ap.parse_args()
    snap=json.loads((DATA/"ethereum.json").read_bytes());reader=Reader("ethereum",snap)
    result={"directory":directory(reader,snap),"payer":OWNER,"frames":[capture_frame(reader,x) for x in FRAME_TX]}
    args.output.write_text(json.dumps(result,separators=(",",":")),encoding="utf8")
