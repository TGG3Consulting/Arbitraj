"""Capture contract identities and bindings on the same canonical parents as the funded bopAMM routes."""
import gzip
import json
import sys
from pathlib import Path

from eth_utils import keccak
from record_bopamm_cash import DATA,FACTORY,UNI,USDC,USDT,WETH,node_tx
from fork_fee_rpc import Reader

def capture(reader, frame, quoter):
    tag={"blockHash":frame["parent"],"requireCanonical":True}
    pool=frame["stable_pool"]
    addresses=dict(weth=WETH,usdc=USDC,usdt=USDT,pool=pool,router=UNI,quoter=quoter,factory=FACTORY)
    codes=reader.many([("eth_getCode",[a,tag]) for a in addresses.values()])
    assert all(c!="0x" for c in codes)
    fee=frame["stable_fee"]
    queries=[
        (UNI,"factory()",[],[],FACTORY),(quoter,"factory()",[],[],FACTORY),(pool,"factory()",[],[],FACTORY),
        (pool,"token0()",[],[],USDC),(pool,"token1()",[],[],USDT),(pool,"fee()",[],[],fee),
        (FACTORY,"getPool(address,address,uint24)",["address","address","uint24"],[USDC,USDT,fee],pool)]
    calls=[node_tx(a,n,types,args) for a,n,types,args,_ in queries]
    answers=reader.many([("eth_call",[call,tag]) for call in calls])
    for (*_,expected),answer in zip(queries,answers,strict=True):
        assert int(answer,16)==(int(expected,16) if isinstance(expected,str) else expected)
    return dict(selector=tag,fee=fee,roles={name:dict(address=a,hash="0x"+keccak(bytes.fromhex(code[2:])).hex())
        for (name,a),code in zip(addresses.items(),codes,strict=True)},calls=calls,answers=answers)

if __name__=="__main__":
    # Ethereum Quoter V1 in Uniswap v3 official Ethereum deployments.
    quoter="0xb27308f9f90d607463bb33ea1bebb41c27ce5ab6"
    reader=Reader("ethereum",json.loads((DATA/"ethereum.json").read_bytes()))
    frames=json.loads(gzip.decompress((DATA/"bopamm_ethereum_cash_wallet.json.gz").read_bytes()))["frames"]
    by_parent={r["parent"]:r for r in frames}
    out=[capture(reader,f,quoter) for f in by_parent.values()]
    assert len(out)==2 and out[0]["roles"]==out[1]["roles"]
    Path(sys.argv[1]).write_text(json.dumps(out,separators=(",",":")))
