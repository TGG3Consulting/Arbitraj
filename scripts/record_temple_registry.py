"""Read TempleDAO venue roles, registered cash assets and both sides of the integer treasury boundary."""
import argparse
import json
from pathlib import Path
from eth_abi import decode
from eth_utils import keccak
from record_base_slipstream_executor import calldata
from record_carbon_registry import Snapshot
from record_ethereum_x7 import ROOT_HASH
from record_temple_ethereum import PAIR, ROUTER, TEMPLE, FRAX, DAI

def capture():
    base=Path(__file__).resolve().parents[1]/"tests/data/first_run_dex_2026-11-26/ethereum.json"
    profile=json.loads(base.read_text(encoding="utf8"))
    chain=Snapshot({"height":profile["block"],"selector":profile["selector"],"state_root":ROOT_HASH})
    calls=[]
    def get(addr,sig,types,args,returns):
        data=calldata(sig,types,args)
        raw=chain.rpc("eth_call",[{"to":addr,"data":data},chain.tag])
        vals=decode(returns,bytes.fromhex(raw[2:]));calls.append({"address":addr,"method":sig,"data":data,"result":raw})
        return vals[0] if len(vals)==1 else vals
    def address(addr,sig):return get(addr,sig,[],[],["address"])
    contracts={}
    for a in [PAIR,ROUTER]:
        code=chain.code(a);contracts[a]={"bytes":len(code),"keccak256":"0x"+keccak(code).hex()}
    if (address(PAIR,"router()"),address(ROUTER,"templeToken()"),address(PAIR,"token0()"),address(PAIR,"token1()"))!=(ROUTER,TEMPLE,TEMPLE,FRAX):raise ValueError("pair and router no longer match")
    treasury=address(ROUTER,"templeTreasury()");defend=address(ROUTER,"defendStable()")
    anchors={s:get(ROUTER,"tokenPair(address)",["address"],[s],["address"]) for s in [FRAX,DAI]}
    if anchors[FRAX]!=PAIR:raise ValueError("retired seed")
    available=get(defend,"balanceOf(address)",["address"],[ROUTER],["uint256"])
    reserves=list(get(PAIR,"getReserves()",[],[],["uint112","uint112","uint32"]))[:2]
    balances=[get(t,"balanceOf(address)",["address"],[PAIR],["uint256"]) for t in [TEMPLE,FRAX]]
    rate=get(treasury,"intrinsicValueRatio()",[],[],["uint256","uint256"])
    r0,r1=reserves;f,t=rate
    crossing=(995*r1*t-1000*r0*f)//(995*f)
    lots=sorted({1,2,3,10**15,10**18,10**22,*range(crossing-4,crossing+5)})
    results=[]
    for n in lots:
        amm=n*995*r1//(1000*r0+n*995);iv=n*f//t
        branch,out=get(ROUTER,"swapExactTempleForStableQuote(address,uint256)",["address","uint256"],[PAIR,n],["bool","uint256"])
        if branch!=(iv>amm) or out!=max(iv,amm):raise ValueError("router output differs")
        results.append({"input":str(n),"amm":str(amm),"iv":str(iv),"defence":branch,"out":str(out)})
    return {"selector":chain.tag,"state_root":ROOT_HASH,"height":chain.height,"contracts":contracts,"pair":PAIR,"router":ROUTER,"temple":TEMPLE,
        "stable":FRAX,"defend":defend,"treasury":treasury,"owners":[address(PAIR,"owner()"),address(ROUTER,"owner()")],
        "token_pair":anchors,"reserves":list(map(str,reserves)),"balances":list(map(str,balances)),
        "ratio":list(map(str,rate)),"defense_balance":str(available),"boundaries":results,"calls":calls}

if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument("output",type=Path);args=parser.parse_args()
    args.output.write_text(json.dumps(capture(),indent=2)+"\n",encoding="utf8",newline="\n")
