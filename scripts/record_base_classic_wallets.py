"""Compare Base Classic Solidly pool quotes, wallet balances and V5 limits in each registered curve."""
import argparse
import json
from pathlib import Path

from eth_utils import keccak
from record_aero_basic_base import HOLDERS, RUNTIME, round_trip
from record_base_classic_directory import ENDPOINTS, optional
from record_balancer_markets import Capture
from record_base_slipstream_executor import EXEC, OWNER, rpc


def capture(directory, endpoint):
    tag=directory["selector"]
    head=rpc(endpoint,"eth_getBlockByHash",[tag["blockHash"],False])
    if rpc(endpoint,"eth_chainId",[])!="0x2105" or head["stateRoot"]!=directory["state_root"]:
        raise ValueError("wrong archive")
    e=dict(endpoint=endpoint,selector=tag)
    runtime=RUNTIME.read_text().strip().removeprefix("0x")
    deadline=int(head["timestamp"],16)+120
    # This address supplied the archived Archly v1 stable pair in transaction 0x4a4b...63ff.
    # Its accepted header has an EIP-7702 delegation and a real wTV balance.
    holders={**HOLDERS["base"], "0x174d6d746503d897acec6e65dc57c024dd6ad4b7":"0x389bd7cad27acc90adef1d23ee2e8cc8608dcc18"}
    potential=[]
    for market in directory["markets"]:
        if market.get("fixed_fee")=="0" and int(market.get("fee_manager","0"),16): continue
        if not market["current_init"] or market["balances"]!=market["reserves"] or not all(map(int,market["reserves"])) or market["fee"] is None: continue
        for i, token in enumerate(market["tokens"]):
            if token not in holders: continue
            scale=int(market["scales"][i])
            reserve=int(market["reserves"][i])
            sizes=sorted({min(max(scale//2000,1),reserve//10000), min(max(scale//200,1),reserve//1000)})
            if market["stable"]:
                sizes=sorted({min(max(scale//2000,1),reserve//50),min(max(scale//200,1),reserve//20)})
            if len(sizes)==2 and sizes[0]>0:
                potential.append((reserve*10**18//scale,market,i,sizes))
    c=Capture(e)
    prescreen=optional(c,[(m["pool"],"getAmountOut(uint256,address)",["uint256","address"],[qty,m["tokens"][i]])
                          for _,m,i,sizes in potential for qty in sizes])
    potential=[p for i,p in enumerate(potential) if all(v and int.from_bytes(v,"big") for v in prescreen[2*i:2*i+2])]
    tested=[]
    failures=[]
    signers={}
    for stable in (False,True):
        ranked=sorted((p for p in potential if p[1]["stable"]==stable),key=lambda p:-p[0])
        for _,m,i,sizes in ranked[:12]:
            a,b=m["tokens"][i],m["tokens"][1-i]
            signer=holders[a]
            if signer not in signers:
                code=rpc(endpoint,"eth_getCode",[signer,tag])
                signers[signer]=code=="0x" or (len(code)==48 and code.startswith("0xef0100"))
            if not signers[signer]: continue
            try:
                cases=[round_trip(e,m["pool"],a,b,qty,deadline,runtime,kind=3,owner=signer) for qty in sizes]
                if a=="0x174d6d746503d897acec6e65dc57c024dd6ad4b7":
                    # Fund an unconnected signer through ERC20.transfer; its own approve and two swaps follow.
                    if rpc(endpoint,"eth_getCode",[OWNER,tag])!="0x" or c.one(a,"balanceOf(address)",["address"],[OWNER],["uint256"])!=(0,) or c.one(a,"allowance(address,address)",["address","address"],[OWNER,EXEC],["uint256"])!=(0,):
                        raise ValueError("unconnected wallet does not start with verified empty token balance and approval")
                    for case,qty in zip(cases,sizes,strict=True):
                        witness=round_trip(e,m["pool"],a,b,qty,deadline,runtime,kind=3,owner=OWNER,funding=signer)
                        if (case["bought"],case["sold"])!=(witness["bought"],witness["sold"]):
                            raise ValueError("wTV historical and unconnected payer wallets receive different amounts")
                        case["unconnected_wallet"]=witness
            except ValueError as err:
                failures.append(dict(pool=m["pool"],stable=stable,token_in=a,reason=str(err)[:380]))
                continue
            tested.append(dict(pool=m["pool"],stable=stable,token_in=a,token_out=b,cases=cases))
            break
    if not tested and potential and not directory["factory_paused"]:
        raise ValueError(f"no complete round trip; {str(failures)[:580]}")
    return dict(id=directory["id"],selector=tag,state_root=head["stateRoot"],v5_hash="0x"+keccak(hexstr=runtime).hex(),
                markets=tested,rejected=failures,candidates=len(potential))


if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument("directory",type=Path);p.add_argument("output",type=Path);args=p.parse_args()
    directory=json.loads(args.directory.read_bytes())
    for endpoint in ENDPOINTS:
        try:
            result=capture(directory,endpoint)
            args.output.write_text(json.dumps(result,indent=2)+"\n",encoding="utf8",newline="\n")
            break
        except (ValueError,KeyError,OSError,RuntimeError) as error:
            print(endpoint,str(error)[:700],flush=True)
    else: raise ValueError("no endpoint returned wallet measurements")
