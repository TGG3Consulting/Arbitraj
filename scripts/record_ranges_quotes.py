"""Record exact-input and exact-output quotes, including physical reserve boundaries."""
import argparse
import gzip
import json
from pathlib import Path
from time import sleep

import requests
from concurrent.futures import ThreadPoolExecutor

from eth_abi import decode
from record_balancer_markets import Capture
from record_base_slipstream_executor import OWNER, ROOT
from record_ranges_ethereum import AT, ENDPOINT

DIRECTORY = ROOT / "tests/data/first_run_dex_2026-11-26/ranges_ethereum_directory.json.gz"
ARGS = ["address","address","address","uint256","address","bytes"]


def grid(p):
    for i, a in enumerate(p["tokens"]):
        for j, b in enumerate(p["tokens"]):
            if i == j:continue
            for exact, side in (("In",i),("Out",j)):
                balance = p["raw"][side]
                sizes = {max(1,balance//100), max(1, balance*2//3)}
                if (i,j)==(0,1) or (i,j)==(1,0):
                    sizes.update([1,max(1,balance//10**8),balance,balance+1,2*balance,50*balance])
                    cap=max(0,p["live"][side]-10**6)//p["scales"][side]
                    sizes.update([max(1,cap-1),max(1,cap),cap+1])
                sig=f"querySwapSingleTokenExact{exact}(address,address,address,uint256,address,bytes)"
                for n in sorted(sizes):
                    yield [i,j,exact,n],(sig,ARGS,[p["pool"],a,b,n,OWNER,b""])


def quotes(endpoint):
    directory=json.loads(gzip.decompress(DIRECTORY.read_bytes()))
    router=directory["roles"]["router"]
    def capture(p):
        c=Capture(dict(endpoint=endpoint,selector=AT))
        rows=[];cases=list(grid(p))
        for k in range(0,len(cases),8):
            group=cases[k:k+8]
            # Vault.quote writes ephemeral virtual balances. Every row starts from the same parent.
            for retry in range(9):
                c.endpoint=(endpoint,"https://rpc.mevblocker.io","https://eth-mainnet.public.blastapi.io")[retry%3]
                pos=len(c.calls)
                try:
                    out=c.batch([(router,*op) for _,op in group])
                    if all(data is not None or (isinstance(ans["response"],dict)
                           and ans["response"].get("code")==3) for data,ans in zip(out,c.calls[pos:],strict=True)):
                        break
                except (requests.RequestException,ValueError):
                    if retry==8:raise
                sleep(min(1+retry,5))
            else: raise RuntimeError(("archive RPC temporarily throttled",p["pool"],k))
            for (key,_),data,answer in zip(group,out,c.calls[pos:],strict=True):
                if data is not None:
                    value,=decode(["uint256"],data)
                    rows.append([*key,value])
                else:
                    error=answer["response"]
                    if not isinstance(error,dict) or error.get("code")!=3 or len(error.get("data",""))<10:
                        raise ValueError(("unconfirmed pool revert",p["pool"],key,error))
                    rows.append([*key,error["data"][:10]])
        return p["pool"],rows
    with ThreadPoolExecutor(max_workers=2) as workers:
        rows=dict(workers.map(capture,[p for p in directory["pools"] if p["initialized"]]))
    return dict(selector=AT,router=router,owner=OWNER,markets=rows)


if __name__=="__main__":
    a=argparse.ArgumentParser(description=__doc__);a.add_argument("out",type=Path)
    a.add_argument("--endpoint",default=ENDPOINT);args=a.parse_args()
    data=quotes(args.endpoint)
    args.out.write_bytes(gzip.compress(json.dumps(data,separators=(",",":")).encode(),mtime=0))
    print("Recorded",sum(map(len,data["markets"].values())),"on-chain quotes")
