"""Compare Base Classic Solidly pool quotes, wallet balances and V5 limits in each registered curve."""
import argparse
import json
from pathlib import Path

from eth_utils import keccak
from record_aero_basic_base import HOLDERS, RUNTIME, round_trip
from record_base_classic_directory import ENDPOINTS
from record_base_slipstream_executor import rpc


def capture(directory, endpoint):
    tag=directory["selector"]
    head=rpc(endpoint,"eth_getBlockByHash",[tag["blockHash"],False])
    if rpc(endpoint,"eth_chainId",[])!="0x2105" or head["stateRoot"]!=directory["state_root"]:
        raise ValueError("wrong archive")
    e=dict(endpoint=endpoint,selector=tag)
    runtime=RUNTIME.read_text().strip().removeprefix("0x")
    deadline=int(head["timestamp"],16)+120
    potential=[]
    for market in directory["markets"]:
        if not market["current_init"] or market["balances"]!=market["reserves"] or not all(map(int,market["reserves"])) or market["fee"] is None: continue
        for i, token in enumerate(market["tokens"]):
            if token not in HOLDERS["base"]: continue
            scale=int(market["scales"][i])
            reserve=int(market["reserves"][i])
            sizes=sorted({min(max(scale//2000,1),reserve//10000), min(max(scale//200,1),reserve//1000)})
            if len(sizes)==2 and sizes[0]>0:
                potential.append((reserve*10**18//scale,market,i,sizes))
    tested=[]
    failures=[]
    signers={}
    for stable in (False,True):
        ranked=sorted((p for p in potential if p[1]["stable"]==stable),key=lambda p:-p[0])
        for _,m,i,sizes in ranked[:12]:
            a,b=m["tokens"][i],m["tokens"][1-i]
            signer=HOLDERS["base"][a]
            if signer not in signers: signers[signer]=rpc(endpoint,"eth_getCode",[signer,tag])=="0x"
            if not signers[signer]: continue
            try:
                cases=[round_trip(e,m["pool"],a,b,qty,deadline,runtime,kind=3) for qty in sizes]
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
