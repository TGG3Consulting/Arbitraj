"""Два рынка Stabull в одной транзакции штатного Router или V5. Оба входа приобретает сам плательщик."""
import argparse
import gzip
import json
from pathlib import Path

from eth_abi import encode
from eth_utils import keccak
from record_base_slipstream_executor import ROOT
from record_stabull_wallets import DATA, EXEC, MODULE, OK, ORDER, OWNER, RUNTIME, STEP, SWAP, WORD, call, calldata, rpc, request

TRADE = "0x" + keccak(text="Trade(address,address,address,uint256,uint256,int128)").hex()


def capture():
    native = json.loads(gzip.decompress((DATA/"stabull_ethereum_native_wallets.json.gz").read_bytes()))
    directory = json.loads(gzip.decompress((DATA/"stabull_ethereum_directory.json.gz").read_bytes()))
    plan = json.loads((ROOT/"ui/src/assets/stabull-ethereum.json").read_bytes())
    chain = json.loads((DATA/"ethereum.json").read_bytes())
    tag, url = chain["selector"], chain["endpoint"]
    assert tag == directory["anchor"] == native["selector"] and plan["blockHash"] == tag["blockHash"]
    router = directory["factory"]["router"]
    stamp = native["simulated_at"]
    weth = "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"
    code = request(url,"eth_call",[{"from":OWNER,"data":plan["data"]},tag])
    assert "0x"+keccak(hexstr=code).hex() == plan["runtimeKeccak"]
    states = {OWNER:{"balance":hex(2*10**18)}, MODULE:{"code":code},
              EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
                    "stateDiff":{WORD(0):WORD(int(OWNER,16)),WORD(1):WORD(int(weth,16))}}}
    for a in states:
        assert rpc(url,"eth_getCode",[a,tag]) == "0x"
    def tx(to, sig, types, args):
        return call(to,calldata(sig,types,args),gas=5_000_000)
    def approve(token,spender,amount):
        return tx(token,"approve(address,uint256)",["address","uint256"],[spender,amount])
    def run(calls):
        [child] = rpc(url,"eth_simulateV1",[{"blockStateCalls":[{"blockOverrides":{"time":hex(stamp)},"stateOverrides":states,"calls":calls}]},tag])
        assert child["parentHash"] == tag["blockHash"] and len(child["calls"]) == len(calls)
        return child["calls"]
    funded = []
    for row in native["records"]:
        one = next(c["buy"] for c in row["cases"] if c["mode"]=="pool" and c["size_usdc"]==1_000_000)
        if one["stage"] == "paid":
            funded.append((row["pool"],one))
    assert len(funded)==2
    cases=[]
    for (first,acquired),(second,other) in [funded,list(reversed(funded))]:
        cash,source,output=acquired["input"],acquired["output"],other["output"]
        quantity=int(acquired["net"])
        prefix=native["funding"]+[approve(cash,first,int(acquired["paid"])),acquired["calldata"]]
        roles=[OWNER,EXEC,MODULE,router]
        reads=[tx(token,"balanceOf(address)",["address"],[a]) for a in roles for token in (source,output,cash)]
        for mode,target in [("router",router),("v5",EXEC)]:
            budget=prefix+[approve(source,target,quantity)]
            def order(minimum,deadline):
                if mode=="router":
                    return tx(target,"originSwap"+SWAP,ORDER,[cash,source,output,quantity,minimum,deadline])
                steps=[(17,MODULE,source,cash,10000,encode(["address"],[first])),
                       (17,MODULE,cash,output,10000,encode(["address"],[second]))]
                return tx(target,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                          ["address","uint256","address","uint256","uint256",STEP+"[]"],[source,quantity,output,minimum,deadline,steps])
            probe=run(budget+reads+[order(1,stamp+240)]+reads)
            if not OK(probe[-len(reads)-1]):
                assert all(map(OK,probe[:len(budget)+len(reads)])) and all(map(OK,probe[-len(reads):]))
                unchanged=lambda seq:[v["returnData"] for v in seq]
                assert unchanged(probe[len(budget):len(budget)+len(reads)])==unchanged(probe[-len(reads):])
                cases.append(dict(mode=mode,pools=[first,second],assets=[source,output,cash],available=False,
                                  error=probe[-len(reads)-1].get("error")))
                continue
            assert all(map(OK,probe))
            balances=lambda seq:[int(v["returnData"],16) for v in seq]
            before=balances(probe[len(budget):len(budget)+len(reads)])
            after=balances(probe[-len(reads):])
            paid,net=before[0]-after[0],after[1]-before[1]
            assert paid==quantity and net>0 and before[2:]==after[2:]
            tight=order(net,stamp+240)
            boundary=run(budget+reads+[order(net+1,stamp+240),order(net,stamp-1)]+reads+[tight]+reads)
            p=len(budget)+len(reads)
            failed,late=boundary[p:p+2];settled=boundary[-len(reads)-1]
            assert not OK(failed) and not OK(late) and OK(settled)
            assert all(map(OK,boundary[:p])) and all(map(OK,boundary[p+2:]))
            assert balances(boundary[len(budget):p])==balances(boundary[p+2:p+2+len(reads)])==before
            assert balances(boundary[-len(reads):])==after
            legs=[log for log in settled["logs"] if log["topics"] and log["topics"][0]==TRADE]
            assert [log["address"].lower() for log in legs]==[first,second]
            cases.append(dict(mode=mode,available=True,pools=[first,second],roles=roles,assets=[source,output,cash],spent=str(paid),
                              received=str(net),before=list(map(str,before)),after=list(map(str,after)),gas=int(settled["gasUsed"],16),
                              approval_gas=int(boundary[len(prefix)]["gasUsed"],16),minimum_plus_one=failed.get("error"),
                              expired=late.get("error"),trade=tight,events=legs,logs=settled["logs"]))
    return dict(chain=1,selector=tag,module_hash=plan["runtimeKeccak"],payer=OWNER,simulated_at=stamp,cases=cases)

if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__);p.add_argument("output",type=Path);a=p.parse_args()
    a.output.write_text(json.dumps(capture(),separators=(",",":")),encoding="utf8")

