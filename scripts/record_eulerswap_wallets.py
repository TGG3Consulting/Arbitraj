"""Штатный EulerSwapPeriphery, собственный плательщик модуля и реальный байткод ArbExecutor V5."""
import argparse
import gzip
import json
from pathlib import Path

from eth_abi import encode
from eth_utils import keccak

from record_base_slipstream_executor import ROOT, OWNER, EXEC, RUNTIME, STEP, call, calldata, rpc
from record_eulerswap_directory import DATA, Reader

ORDER = ["address", "address", "address", "uint256", "uint256", "uint256"]
def OK(call_result):
    return call_result["status"]=="0x1"

def WORD(value):
    return "0x"+format(value,"064x")

MODULE = "0x" + keccak(text="EulerSwap test wallet with independent base and ethereum funding")[-20:].hex()
STABLE = "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"
WRAPPED = {"base":"0x4200000000000000000000000000000000000006","ethereum":"0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"}
CASES = {
    "base": [(1,"0x1f23c1a134dcc8b4e7827faa442a38b9ae7328a8"),(2,"0x4687e171bea250e3048d8f426b32cb16be17a8a8")],
    "ethereum": [(1,"0xc88b618c2c670c2e2a42e06b466b6f0e82a6e8a8"),(1,"0x1656b05c40bbaaf9b01fbad50205bd44459d68a8"),(2,"0x0bf9244f5f50d79497f58564ee3ab77ef38e28a8")]}


def tx(address,name,types,args):
    return call(address,calldata(name+"("+",".join(types)+")",types,args),gas=4_000_000)


def capture(network, artifact):
    registry = json.loads(gzip.decompress((DATA/f"eulerswap_{network}_registry.json.gz").read_bytes()))
    url = json.loads((DATA/f"{network}.json").read_bytes())["endpoint"]
    tag = registry["selector"]
    roles = registry["roles"]
    head = rpc(url,"eth_getBlockByHash",[tag["blockHash"],False])
    assert head["hash"] == tag["blockHash"] and int(rpc(url,"eth_chainId",[]),16) == registry["chain"]
    stamp = int(head["timestamp"],16)+2
    expiry = stamp+240
    weth = WRAPPED[network]
    assert all(rpc(url,"eth_getCode",[a,tag])=="0x" for a in [OWNER,EXEC,MODULE])
    factories = [roles[name]["address"] for name in ["V1Factory","V2Factory"]]
    creation = {"from":OWNER,"data":"0x"+artifact["creation"]+encode(["address","address"],factories).hex()}
    raw = bytes.fromhex(Reader(network).rpc("eth_call",[creation,tag])[2:])
    template = bytes.fromhex(artifact["runtime"])
    masked = bytearray(raw)
    bound = set()
    for refs in artifact["immutableReferences"].values():
        values = set()
        for ref in refs:
            start, size = ref["start"],ref["length"]
            assert size == 32
            values.add(raw[start:start+size])
            masked[start:start+size]=template[start:start+size]
        value,=values
        bound.add(value)
    assert masked==template and bound=={encode(["address"],[f]) for f in factories}
    overrides = {OWNER:{"balance":hex(2*10**18)},MODULE:{"code":"0x"+raw.hex()},
                 EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
                       "stateDiff":{WORD(0):WORD(int(OWNER,16)),WORD(1):WORD(int(weth,16))}}}
    def run(actions):
        block={"blockOverrides":{"time":hex(stamp)},"stateOverrides":overrides,"calls":actions}
        child,=rpc(url,"eth_simulateV1",[{"blockStateCalls":[block]},tag])
        assert child["parentHash"]==tag["blockHash"] and len(child["calls"])==len(actions)
        return child["calls"]
    def tokens(a,b,spender):
        reads=[tx(token,"balanceOf",["address"],[account]) for account in [OWNER,MODULE,EXEC] for token in [a,b]]
        return reads+[tx(a,"allowance",["address","address"],[OWNER,spender])]
    profiles=json.loads((ROOT/"config/first_run_dex.json").read_bytes())
    v2id="uniswap-v2-base" if network=="base" else "uniswap_v2"
    native_router=next(p["settings"]["dex.router"] for p in profiles if p["id"]==v2id and p["network"]==network)
    def funding(asset):
        if asset==weth:
            return call(weth,calldata("deposit()",[],[]),10**17)
        if network=="base" and asset.startswith("0x2ae3f1ec"):
            # Официальный SwapRouter02 Base, WETH -> cbETH за собственный ETH; пул по реально активному tier.
            router02="0x2626664c2603336e57b271c5c0b26f421741e481"
            for tier in (100,500,3000,10000):
                params="(address,address,uint24,address,uint256,uint256,uint160)"
                order=call(router02,calldata("exactInputSingle("+params+")",[params],[(weth,asset,tier,OWNER,10**16,10**15,0)]),10**16)
                if OK(run([order])[0]):
                    return order
            raise ValueError("Не найдено штатного финансирования cbETH через Uniswap V3")
        ftypes=["uint256","address[]","address","uint256"]
        return call(native_router,calldata("swapExactETHForTokens("+",".join(ftypes)+")",ftypes,
                                      [15_000_000,[weth,asset],OWNER,expiry]),10**17)
    cases=[]
    refusals=[]
    for version,pool in CASES[network]:
        p=next(p for p in registry["generations"][version-1]["pools"] if p["address"]==pool)
        router=roles[f"V{version}Periphery"]["address"]
        assert p["installed"]
        start=weth if weth in p["tokens"] else STABLE
        other=next(t for t in p["tokens"] if t!=start)
        fund=funding(start)
        sizes=[10**14,10**15] if start==weth else [10**6,10**7]
        for mode in ["official","module","v5"]:
            spender={"official":router,"module":MODULE,"v5":EXEC}[mode]
            for quantity in sizes:
                for wanted in ([False] if mode=="v5" else [False,True]):
                    prefix=[fund]
                    amount=quantity
                    for a,b in [(start,other),(other,start)]:
                        def quote(x,exact):
                            out=run(prefix+[tx(pool,"computeQuote",["address","address","uint256","bool"],[a,b,x,exact])])
                            assert all(map(OK,out))
                            return int(out[-1]["returnData"],16)
                        target=quote(amount,True)
                        if wanted:
                            target=max(1,target//2)
                            cost=quote(target,False)
                        else:
                            cost=amount
                        assert target>0 and 0<cost<=amount
                        auth=tx(a,"approve",["address","uint256"],[spender,cost])
                        def order(limit,deadline):
                            if mode=="official":
                                return tx(router,"swapExactOut" if wanted else "swapExactIn",
                                          ["address","address","address","uint256","address","uint256","uint256"],
                                          [pool,a,b,target if wanted else cost,OWNER,limit,deadline])
                            if mode=="module":
                                return tx(MODULE,"executeTarget" if wanted else "execute",ORDER,
                                          [pool,a,b,limit if wanted else cost,target if wanted else limit,deadline])
                            return tx(EXEC,"execute",["address","uint256","address","uint256","uint256",STEP+"[]"],
                                      [a,cost,b,limit,deadline,[(17,MODULE,a,b,10000,encode(["address"],[pool]))]])
                        bound=cost if wanted else target
                        strict=order(bound-1 if wanted else bound+1,expiry)
                        normal=order(bound,expiry)
                        before=tokens(a,b,spender)
                        plan=prefix+[auth]+before+[strict,order(bound,stamp-1),normal]+before
                        out=run(plan)
                        i=len(prefix)+1
                        assert all(map(OK,out[:i+len(before)]))
                        old=[int(x["returnData"],16) for x in out[i:i+len(before)]]
                        stricter,expired,fill=out[i+len(before):i+len(before)+3]
                        assert not OK(stricter) and not OK(expired)
                        ends=out[-len(before):]
                        assert all(map(OK,ends))
                        new=[int(x["returnData"],16) for x in ends]
                        if not OK(fill):
                            assert old==new
                            refusals.append(dict(version=version,pool=pool,mode=mode,wanted=wanted,input=a,output=b,
                                amount=amount,minimum=target,quote_cost=cost,error=fill.get("error"),before=old,after=new))
                            if a==start:
                                prefix=[funding(b)]
                                decimals=p["decimals"][p["tokens"].index(b)]
                                amount=(10**(decimals-3) if decimals>=12 else 10**decimals)//(10 if quantity==sizes[0] else 1)
                            continue
                        paid=old[0]-new[0];net=new[1]-old[1]
                        assert paid==cost and net>=target and old[2:6]==new[2:6] and new[-1]==0
                        cases.append(dict(version=version,pool=pool,mode=mode,wanted=wanted,input=a,output=b,
                            amount=amount,minimum=target,paid=paid,net=net,payer=OWNER,spender=spender,
                            funding=prefix[0],order=normal,strict=stricter.get("error"),expired=expired.get("error"),before=old,after=new,
                            logs=fill["logs"],gas=int(fill["gasUsed"],16),approval_gas=int(out[i-1]["gasUsed"],16)))
                        prefix.extend([auth,normal])
                        amount=net
    return dict(chain=registry["chain"],selector=tag,stamp=stamp,module=MODULE,executor=EXEC,payer=OWNER,
                runtime="0x"+keccak(raw).hex(),factories=factories,solc=artifact["solc"],cases=cases,refusals=refusals)


if __name__=="__main__":
    ap=argparse.ArgumentParser()
    ap.add_argument("network",choices=CASES)
    ap.add_argument("artifact",type=Path)
    ap.add_argument("output",type=Path)
    args=ap.parse_args()
    args.output.write_text(json.dumps(capture(args.network,json.loads(args.artifact.read_bytes())),separators=(",",":")))

