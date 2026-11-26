"""Reproduce official V2 Router wallet debits, recipient balances and atomic limits at the archive block."""
import argparse
import gzip
import json
from pathlib import Path

from eth_abi import decode
from eth_utils import keccak
from community_v3_rpc import ENDPOINTS
from derpdex_archive_rpc import Readers
from record_base_slipstream_executor import OWNER, ROOT, call, calldata, simulate_block
from record_ekubo_v2_ethereum import CORE, ROUTER, raw


ETH = "0x" + "0"*40
MULTICALL3 = "0xca11bde05977b3631167028862be2a173976ca11"
FIXTURE = ROOT/"tests/data/first_run_dex_2026-11-26/ekubo_v2_ethereum_activity.json.gz"
KEY = "(address,address,bytes32)"
NODE = "("+KEY+",uint96,uint256)"
ORDER = "("+NODE+"[],(address,int128))"
# Public EOA holders are solely a source of actual tokens in the isolated simulation.
HOLDERS = {
    "0x514910771af9ca656af840dff83e8264ecf986ca":"0xf977814e90da44bfa03b6295a0616a897441acec",
    "0x6440f144b7e50d6a8439336510312d2f54beb01d":"0xd44b43269e49cdbc763a45816cc0eb54ce3c0483",
    "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48":"0x37305b1cd40574e4c5ce33f8e8306be057fd7341",
    "0xcbb7c0000ab88b473b1f5afd9ef808440eed33bf":"0xb561b1e79e448bc3a7ac4dc377eeecbc99505f60",
    "0xdac17f958d2ee523a2206206994597c13d831ec7":"0xf977814e90da44bfa03b6295a0616a897441acec",
    "0x2260fac5e5542a773aa44fbcfedf7c193bc2c599":"0x176f3dab24a159341c0509bb36b833e7fdd0a132",
}


def capture(source):
    tag=source["selector"]
    height=source["window"][-1]["number"]
    marker="0x"+keccak(text="V2 archival marker")[-20:].hex()
    state={marker:{"code":"0x602a60005260206000f3"}}
    reader=Readers(tag,height,source["state_root"],CORE,state,{"to":marker,"data":"0x"},
                   "0x"+format(42,"064x"),urls=ENDPOINTS,chain=1,word_reads=False)
    rpc=reader.read
    header=rpc(None,"eth_getBlockByHash",[tag["blockHash"],False])
    for name in ("core","router"):
        a=source[name]["address"]
        if "0x"+keccak(raw(rpc(None,"eth_getCode",[a,tag]))).hex()!=source[name]["runtime"]:
            raise ValueError("deployment identity changed")
    deadline=int(header["timestamp"],16)+600
    balance=lambda asset:call(asset,calldata("balanceOf(address)",["address"],[OWNER])) if asset!=ETH else call(
        MULTICALL3,calldata("getEthBalance(address)",["address"],[OWNER]))
    overrides={OWNER:{"balance":hex(10**20)}}
    cases=[]
    for m in source["markets"]:
        k=m["key"]
        key=(k["token0"],k["token1"],raw(k["config"]))
        if int(k["extension"],16):raise ValueError("unsupported extension")
        samples=[(row["direction"],int(row["amount"])) for row in m["quotes"]]
        for row in m["quotes"]:
            if not row["full"]:
                available=int(row["delta"+str(row["direction"])])
                if available>4:
                    samples.extend((row["direction"],n) for n in (available//4,available//2))
        for side,amount in dict.fromkeys(samples):
            a,b=key[side],key[1-side]
            limit=79227682466138141934206691491 if side else 4611797791050542631
            q=calldata("quote("+KEY+",bool,int128,uint96,uint256)",[KEY,"bool","int128","uint96","uint256"],
                       [key,bool(side),amount,limit,0])
            deltas=decode(["int128","int128"],raw(rpc(None,"eth_call",[{"from":OWNER,"to":ROUTER,"data":q},tag])))
            used,out=deltas[side],-deltas[1-side]
            if out<=0 or not 0<used<=amount: raise ValueError("zero or malformed quote")
            prefix=[]
            if a!=ETH:
                holder=HOLDERS[a]
                if raw(rpc(None,"eth_getCode",[holder,tag])): raise ValueError("funding holder is a contract")
                prefix.append({**call(a,calldata("transfer(address,uint256)",["address","uint256"],[OWNER,amount])),"from":holder})
                prefix.append(call(a,calldata("approve(address,uint256)",["address","uint256"],[ROUTER,amount])))
            def swap(minimum):
                ops=[calldata("checkDeadline(uint256)",["uint256"],[deadline]),
                     calldata("recordBalanceForSlippageCheck(address)",["address"],[b]),
                     calldata("multihopSwap("+ORDER+",int256)",[ORDER,"int256"],[([(key,limit,0)],(a,amount)),0]),
                     calldata("checkMinimumOutputReceived(address,uint256)",["address","uint256"],[b,minimum])]
                return call(ROUTER,calldata("multicall(bytes[])",["bytes[]"],[[raw(op) for op in ops]]),
                            value=amount if a==ETH else 0,gas=3500000)
            trial=dict(id=m["id"],side=side,amount=str(amount),used=str(used),out=str(out),sender=OWNER)
            for label,minimum in [("exact",out),("above",out+1)]:
                order=swap(minimum)
                calls=prefix+[balance(a),balance(b),order,balance(a),balance(b)]
                replies=simulate_block(None,tag,overrides,calls,read_rpc=rpc)
                if any(int(row["status"],16)!=1 for i,row in enumerate(replies) if i!=len(prefix)+2):
                    raise ValueError(f"funding or balance inspection failed in {m['id']}, side {side}")
                before_a,before_b,_,after_a,after_b=replies[-5:]
                values=[int(v["returnData"],16) for v in (before_a,before_b,after_a,after_b)]
                spent,received=values[0]-values[2],values[3]-values[1]
                result=replies[-3]
                success=int(result["status"],16)==1
                expected=used==amount and minimum==out
                if success!=expected or (spent,received)!=((amount,out) if expected else (0,0)):
                    raise ValueError(f"{m['id']} {side} {amount}: {label} status={success}, delta={(spent,received)}, error={result.get('error')}")
                if not success:
                    selector="PartialSwapsDisallowed()" if used!=amount else "MinimumOutputNotReceived(address,uint256)"
                    revert=raw(result.get("returnData","0x")) or raw(result.get("error",{}).get("data","0x"))
                    expected_error=keccak(text=selector)[:4]
                    if used==amount:
                        expected_error+=bytes.fromhex(b[2:]).rjust(32,b"\x00")+minimum.to_bytes(32,"big")
                    if revert!=expected_error:
                        raise ValueError(f"unexpected error: {result}")
                trial[label]={"tx":order,"result":result,"balances":list(map(str,values))}
            cases.append(trial)
    return dict(selector=tag,state_root=source["state_root"],endpoints=reader.urls,completed=reader.counts,cases=cases)


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output",type=Path)
    args=parser.parse_args()
    result=capture(json.loads(gzip.decompress(FIXTURE.read_bytes())))
    args.output.write_text(json.dumps(result,indent=2)+"\n",encoding="utf8")
