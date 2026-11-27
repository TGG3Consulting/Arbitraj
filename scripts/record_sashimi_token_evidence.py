"""Group Sashimi credits lacking wallet Transfer evidence by canonical ERC20 runtime, without submitting any calls."""
import argparse
import gzip
import json
from pathlib import Path

from eth_utils import keccak
from record_base_slipstream_executor import rpc
from record_ethereum_x7 import ENDPOINTS

TRANSFER="0x"+keccak(text="Transfer(address,address,uint256)").hex()


def capture(source, endpoint):
    selector=source["selector"]
    header=rpc(endpoint,"eth_getBlockByHash",[selector["blockHash"],False])
    if header["stateRoot"]!=source["state_root"] or int(header["number"],16)!=source["height"] or rpc(endpoint,"eth_chainId",[])!="0x1":
        raise ValueError("wrong canonical state")
    groups={}
    codes={}
    for index,row in enumerate(source["wallets"]):
        buy=row["buy"]
        if buy["stage"]!="paid": continue
        token=buy["tail"][-1]["to"]
        logs=buy["results"][6]["logs"]
        topic="0x"+row["payer"][2:].zfill(64)
        if any(l["address"]==token and len(l["topics"])==3 and l["topics"][0]==TRANSFER and l["topics"][2]==topic for l in logs): continue
        reverse=row["sell"]
        if reverse["stage"]!="allowance_or_quote" or [r["status"] for r in reverse["result"]]!=["0x0","0x1"]:
            raise ValueError(("different reverse outcome",index))
        if token not in codes:
            runtime=rpc(endpoint,"eth_getCode",[token,selector])
            if runtime=="0x":raise ValueError("token code disappeared")
            digest="0x"+keccak(hexstr=runtime).hex();codes[token]=digest
            groups.setdefault(digest,{"runtime":runtime,"tokens":{}})
        groups[codes[token]]["tokens"].setdefault(token,[]).append(index)
    return dict(selector=selector,height=source["height"],state_root=source["state_root"],groups=groups)


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__); parser.add_argument("wallets",type=Path);parser.add_argument("output",type=Path)
    args=parser.parse_args();source=json.loads(gzip.decompress(args.wallets.read_bytes()));reasons=[]
    for node in ENDPOINTS:
        try: result=capture(source,node);break
        except (ValueError,KeyError,OSError,RuntimeError) as error:reasons.append((node,str(error)[:180]))
    else:raise RuntimeError(reasons)
    args.output.write_text(json.dumps(result,indent=2)+"\n",encoding="utf8",newline="\n")
