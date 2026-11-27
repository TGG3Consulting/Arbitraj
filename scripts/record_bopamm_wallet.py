"""Replay own ERC20 funds through a slot-bound bopAMM module next to real multi-maker fills."""
import argparse
import gzip
import json
from pathlib import Path

from eth_abi import decode, encode
from eth_utils import keccak
from record_bopamm_ethereum import (AMM, DATA, FILL, FUNDING, MARKET_EVENT, OWNER, PAMM, REGISTRY,
                                    USDC, WETH, approval, balance, raw, signed_update, tx)
from record_base_slipstream_executor import simulate_block
from record_kewlswap_base import create_address
from fork_fee_rpc import Reader

SAMPLES = [
    ("0x872e23fdb4a370d3a08c7f57c00afeebeb5c19e9339506c1b46a7f7b014b1630",
     "0x79b46ab6c7e1b12139bbe73f81f5f126ee06b090ce404fe89eb4eb4f0b3ce272"),
    ("0xafe3528ef012c1b4a140e80549110d8717146c4345abc156ee543f5d6e03c860",
     "0x6025d4b01d5bb2604d96e1789851a5962e508d62c6c1584cb1fed6686405a6ca"),
]
TRANSFER = "0x" + keccak(text="Transfer(address,address,uint256)").hex()
FILLED = "0x" + keccak(text="Filled(address,bytes32,address,address,uint256,uint256)").hex()
ORDER = "(bytes32,bytes32,uint64,uint64,address,address,uint256,uint256)"
def address(topic):
    return "0x" + topic[-40:]

def maker_fills(logs, a, b, receiver, requested, paid):
    market = [x for x in logs if x["address"] == AMM and x["topics"]]
    swap, = [x for x in market if x["topics"][0] == MARKET_EVENT]
    assert [address(x) for x in swap["topics"][2:]] == [a,b]
    assert decode(["uint256","uint256","address"],raw(swap["data"])) == (requested,paid,receiver)
    transfers = [(x["address"],address(x["topics"][1]),address(x["topics"][2]),int(x["data"],16))
                 for x in logs if len(x["topics"]) == 3 and x["topics"][0] == TRANSFER]
    fills = []
    for x in market:
        if x["topics"][0] != FILL:
            continue
        assert [address(v) for v in x["topics"][2:]] == [a,b]
        size, value = decode(["uint256","uint256"],raw(x["data"]))
        maker, = [to for asset,origin,to,qty in transfers if (asset,origin,qty) == (a,AMM,size)]
        assert transfers.count((b,maker,receiver,value)) == 1
        fills.append(dict(id=int(x["topics"][1],16),maker=maker,spent=str(size),received=str(value)))
    assert sum(int(f["spent"]) for f in fills) == requested and sum(int(f["received"]) for f in fills) == paid
    return fills

def capture(reader, artifact, price, historical):
    h, tag, update = signed_update(reader,price)
    receipt = reader.one("eth_getTransactionReceipt",[historical])
    assert receipt["blockHash"] == h["hash"] and receipt["status"] == "0x1"
    assert h["transactions"].index(price) < h["transactions"].index(historical)
    event, = [x for x in receipt["logs"] if x["address"] == AMM and x["topics"][:1] == [MARKET_EVENT]]
    qty, paid, receiver = decode(["uint256","uint256","address"],raw(event["data"]))
    included = maker_fills(receipt["logs"],WETH,USDC,receiver,qty,paid)
    assert len({f["maker"] for f in included}) >= 2
    height, timestamp = int(h["number"],16), int(h["timestamp"],16)
    block = {"time":h["timestamp"],"feeRecipient":h["miner"],"gasLimit":h["gasLimit"]}
    module,cases = wallet_trials(reader,artifact,tag,height,timestamp,block,
        [(WETH,USDC,qty),(WETH,USDC,10**16),(USDC,WETH,100*10**6),(USDC,WETH,10**6)],update=update)
    return dict(block=height,timestamp=timestamp,hash=h["hash"],parent=tag["blockHash"],update=price,original=historical,
                original_fills=included,original_market_event=event,payer=OWNER,module=module,cases=cases)

def wallet_trials(reader,artifact,tag,height,timestamp,block,amounts,update=None,published=None):
    module = create_address(OWNER,0)
    assert reader.many([("eth_getTransactionCount",[OWNER,tag]),("eth_getCode",[module,tag])]) == ["0x0","0x"]
    create = {"from":OWNER,"gas":hex(5000000),"data":"0x"+artifact["creation"]+encode(
        ["address"]*3,[AMM,PAMM,USDC]).hex()}
    def run(calls, updated=True):
        def request(_url, method, params):
            params[0]["blockStateCalls"][0]["blockOverrides"] = block
            return reader.one(method,params)
        prefix = [update] if updated and update else []
        overrides = {OWNER:{"balance":hex(2*10**18)}}
        if updated and published:
            overrides[REGISTRY] = {"stateDiff":published}
        result = simulate_block(None,tag,overrides,prefix+[create,*calls],read_rpc=request)
        assert all(x["status"] == "0x1" for x in result[:len(prefix)+1])
        runtime = "0x"+keccak(hexstr=result[len(prefix)]["returnData"]).hex()
        return runtime, result[len(prefix)+1:]
    def trial(a,b,size):
        leg = keccak(encode(["address","address","uint256","uint256"],[a,b,size,height]))
        def order(floor, parent=raw(tag["blockHash"]), number=height, slot=timestamp):
            return tx(module,"execute("+ORDER+")",[ORDER],[(leg,parent,number,slot,a,b,size,floor)])
        deposited = tx(WETH,"deposit()",value=2*size) if a == WETH else tx(
            FUNDING,"swapETHForExactTokens(uint256,address[],address,uint256)",
            ["uint256","address[]","address","uint256"],[2*size,[WETH,a],OWNER,timestamp],value=10**18)
        reads = [balance(t,who) for who in [OWNER,AMM,module] for t in [a,b]]
        prefix = [deposited,approval(a,module,2*size),*reads]
        def settled(calls,updated=True):
            runtime, values = run(prefix+calls+reads,updated)
            i = len(prefix)
            assert all(v["status"] == "0x1" for v in values[:i]+values[-len(reads):])
            before = [int(v["returnData"],16) for v in values[2:i]]
            after = [int(v["returnData"],16) for v in values[-len(reads):]]
            assert before[2:] == after[2:]
            return runtime, before[0]-after[0], after[1]-before[1],values[i:-len(reads)]
        def once(updated):
            runtime, spent, net, (result,) = settled([order(1)],updated)
            if result["status"] == "0x0":
                assert (spent,net) == (0,0)
                return dict(status=result["status"],error=result.get("error"),runtime=runtime)
            assert spent == size and net > 0
            return dict(status="0x1",spent=str(spent),net=str(net),runtime=runtime)
        before, after = once(False), once(True)
        if after["status"] == "0x0":
            return dict(input=a,output=b,amount=str(size),before=before,after=after)
        net = int(after["net"])
        invalid = [order(net+1),order(net,number=height+1),order(net,slot=timestamp+12),order(net,parent=bytes(32))]
        executed = order(net)
        runtime, spent, value, results = settled(invalid+[executed,executed])
        assert [r["status"] for r in results] == ["0x0"]*4+["0x1","0x0"]
        assert (spent,value) == (size,net)
        x = results[-2]
        assert decode(["uint256","uint256"],raw(x["returnData"])) == (size,net)
        own, = [l for l in x["logs"] if l["address"] == module and l["topics"][0] == FILLED]
        assert [address(own["topics"][1]),raw(own["topics"][2]),address(own["topics"][3])] == [OWNER.lower(),leg,a]
        assert decode(["address","uint256","uint256"],raw(own["data"])) == (b,size,net)
        after.update(fills=maker_fills(x["logs"],a,b,OWNER.lower(),size,net),gas=int(x["gasUsed"],16),
                     transaction=executed,logs=x["logs"],controls=[r.get("error") for r in results if r["status"] == "0x0"])
        return dict(input=a,output=b,amount=str(size),before=before,after=after)
    return module, [trial(a,b,n) for a,b,n in amounts]

def published_frames(reader,artifact,observations):
    results=[]
    for name in ("BuilderNet","Titan"):
        valid=[row[name] for row in observations if row[name].get("same_live_parent") and
               row[name]["finished_ns"] < (1606824023+row[name]["slot"]*12)*10**9 and
               row[name]["other_patched_accounts"]==0]
        cases=[]
        for sample in valid:
            if len(cases)>=2:
                break
            if any(x["target"]==sample["target"] for x in cases):
                continue
            tag={"blockHash":sample["parent"],"requireCanonical":True}
            head=reader.one("eth_getBlockByHash",[sample["parent"],False])
            slot_time=1606824023+sample["slot"]*12
            assert int(head["number"],16)+1==sample["target"] and int(head["timestamp"],16)<slot_time
            assert sample["registry"]==REGISTRY and sample["key"] in (PAMM,AMM)
            patch=sample["stateDiff"]
            assert 0<len(patch)<=128 and all(len(raw(k))==32 for d in patch.items() for k in d)
            module,legs=wallet_trials(reader,artifact,tag,sample["target"],slot_time,{"time":hex(slot_time)},
                [(WETH,USDC,10**15),(USDC,WETH,10**6)],published=patch)
            cases.append({"target":sample["target"],"slot":sample["slot"],"parent":sample["parent"],
                          "frame_observed_ns":sample["observed_ns"],"module":module,"cases":legs,
                          "source_quotes":sample["quotes"]})
        if len(cases)!=2:
            raise ValueError(f"{name}: insufficient archived canonical frames")
        results.append(dict(builder=name,frames=cases))
    return results

if __name__ == "__main__":
    parser=argparse.ArgumentParser()
    parser.add_argument("output",type=Path)
    parser.add_argument("artifact",type=Path)
    parser.add_argument("--published-output",type=Path)
    args=parser.parse_args()
    reader=Reader("ethereum",json.loads((DATA/"ethereum.json").read_bytes()))
    frames=[capture(reader,json.loads(args.artifact.read_bytes()),*pair) for pair in SAMPLES]
    args.output.write_text(json.dumps(frames,separators=(",",":")),encoding="utf8")
    if args.published_output:
        saved=json.loads(gzip.decompress((DATA/"bopamm_ethereum_frames.json.gz").read_bytes()))
        result=published_frames(reader,json.loads(args.artifact.read_bytes()),saved)
        args.published_output.write_text(json.dumps(result,separators=(",",":")),encoding="utf8")
