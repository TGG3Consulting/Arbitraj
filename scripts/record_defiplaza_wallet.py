"""Сверка чистой оплаты DefiPlaza через защищённый кошелёк и исходный V5 kind 29."""
import argparse
from hashlib import sha256
import gzip
import json
from pathlib import Path

from eth_abi import encode
from eth_utils import keccak
from record_defiplaza import MARKETS, FUNDING, ZERO, WETH, Reader, URLS, tx, read_balance
from record_base_slipstream_executor import EXEC, OWNER, ROOT, RUNTIME, STEP, simulate_block


MODULE = "0x"+keccak(text="DefiPlaza isolated funded wallet").hex()[-40:]


def test_leg(reader, pool, leg, mode, states, deadline):
    source,target=leg["input"],leg["output"]
    a,b=(source,target) if mode=="native" else tuple(WETH if token==ZERO else token for token in (source,target))
    amount,limit=int(leg["raw"]),int(leg["exact"]["received"])
    endpoint=EXEC if mode=="v5" else MODULE
    prefix=[dict(to=MODULE,data="0x",value="0x7",gas="0x186a0",**{"from":OWNER}),
            tx(WETH,"deposit()",value=11),tx(WETH,"transfer(address,uint256)",["address","uint256"],[MODULE,11])]
    if a==WETH:
        prefix.append(tx(WETH,"deposit()",value=amount))
    elif a!=ZERO:
        prefix.append(tx(FUNDING,"swapETHForExactTokens(uint256,address[],address,uint256)",
                         ["uint256","address[]","address","uint256"],[amount,[WETH,a],OWNER,deadline],value=10**18))
    if a!=ZERO:
        prefix.append(tx(a,"approve(address,uint256)",["address","uint256"],[endpoint,amount]))
    marks=[read_balance(t,who) for who,pair in [(OWNER,(a,b)),(pool,(source,target))] for t in pair]
    marks.extend(read_balance(t,MODULE) for t in dict.fromkeys([a,b,ZERO,WETH]))
    if a!=ZERO: marks.append(tx(a,"allowance(address,address)",["address","address"],[MODULE,pool]))
    def action(floor,end):
        if mode=="v5":
            return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                      ["address","uint256","address","uint256","uint256",STEP+"[]"],[a,amount,b,floor,end,[(29,MODULE,a,b,10000,b"")]])
        return tx(MODULE,"execute(address,address,uint256,uint256,uint256)",
                  ["address","address","uint256","uint256","uint256"],[a,b,amount,floor,end],value=amount if a==ZERO else 0)
    trade=action(limit,deadline)
    prefix.append(tx(MODULE,"quote(address,address,uint256)",["address","address","uint256"],[a,b,amount]))
    zero=[] if mode=="v5" else [action(0,deadline)]
    rows=simulate_block(None,reader.tag,states,prefix+zero+marks+[action(limit+1,deadline),action(limit,0),trade]+marks,
                        read_rpc=lambda _,method,p:reader.one(method,p))
    assert int(rows[len(prefix)-1]["returnData"],16)==limit
    if zero: assert rows[len(prefix)]["status"]=="0x0"
    n=len(prefix)+len(zero);m=len(marks)
    before=rows[n:n+m];after=rows[-m:];high,expired,filled=rows[n+m:n+m+3]
    assert all(v["status"]=="0x1" for v in rows[:len(prefix)]+before+after)
    assert [v["status"] for v in (high,expired,filled)]==["0x0","0x0","0x1"], (mode,a,b,amount,filled.get("error"))
    old,new=([int(v["returnData"],16) for v in group] for group in (before,after))
    assert (old[0]-new[0],new[1]-old[1],new[2]-old[2],old[3]-new[3])==(amount,limit,amount,limit) and old[4:]==new[4:]
    return dict(mode=mode,input=a,output=b,spent=str(amount),received=str(limit),before=list(map(str,old)),after=list(map(str,new)),
                gas=int(filled["gasUsed"],16),approval_gas=0 if a==ZERO else int(rows[len(prefix)-2]["gasUsed"],16),
                zero_min=rows[len(prefix)].get("error") if zero else None,plus_one=high["error"],expiry=expired["error"],logs=filled["logs"],action=trade)


def capture(kind, artifact, manifest):
    folder=ROOT/"tests/data/first_run_dex_2026-11-26"
    original=json.loads(gzip.decompress((folder/f"defiplaza_{kind}_ethereum.json.gz").read_bytes()))
    head=json.loads((folder/"ethereum.json").read_bytes())
    URLS["ethereum"]=["https://mainnet.gateway.tenderly.co",*URLS["ethereum"]]
    reader=Reader("ethereum",head)
    assert reader.tag==original["selector"]
    pool=MARKETS[kind][0]
    v5=RUNTIME.read_text().strip().removeprefix("0x")
    abi=["address","address","bool","bytes32"]
    params=[pool,WETH,kind=="stable",keccak(hexstr=v5)]
    data="0x"+artifact["creation"]+encode(abi,params).hex()
    deployed=reader.one("eth_call",[{"from":OWNER,"data":data},reader.tag])
    normalized=bytearray.fromhex(deployed[2:])
    seen=set()
    for refs in artifact["immutableReferences"].values():
        words={bytes(normalized[r["start"]:r["start"]+r["length"]]) for r in refs}
        assert len(words)==1 and all(r["length"]==32 for r in refs)
        seen.update(words)
        for r in refs:normalized[r["start"]:r["start"]+32]=bytes(32)
    assert seen=={encode([t],[v]) for t,v in zip(abi,params,strict=True)} and normalized.hex()==artifact["runtime"]
    word=lambda a:"0x"+format(int(a,16),"064x")
    states={OWNER:{"balance":hex(100*10**18)},MODULE:{"code":deployed},
            EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),"stateDiff":{word("0"):word(OWNER),word("1"):word(WETH)}}}
    assert all(reader.one("eth_getCode",[a,reader.tag])=="0x" for a in states)
    deadline=int(reader.header["timestamp"],16)+300
    fills=[]
    for leg in original["trials"]:
        if leg["exact"].get("status")!="0x1": continue
        modes=["wallet","v5"]
        if ZERO in (leg["input"],leg["output"]): modes.append("native")
        fills.extend(test_leg(reader,pool,leg,mode,states,deadline) for mode in modes)
    if manifest is not None:
        roles={}
        for role,address in [("factory",pool),("wrapped",WETH)]:
            code=reader.one("eth_getCode",[address,reader.tag])
            assert code!="0x"
            roles[role]=dict(address=address,keccak256="0x"+keccak(hexstr=code).hex())
        plan=dict(kind="defiplaza_"+kind,chainId=1,network="Ethereum",sourceBlock=reader.tag["blockHash"],roles=roles,
                  stable=kind=="stable",executorHash="0x"+params[-1].hex(),solc=artifact["solc"],data=data,
                  runtimeKeccak="0x"+keccak(hexstr=deployed).hex(),runtimeSha256=sha256(bytes.fromhex(deployed[2:])).hexdigest(),
                  creationSha256=sha256(bytes.fromhex(data[2:])).hexdigest())
        manifest.write_text(json.dumps(plan,separators=(",",":")),encoding="utf8")
    return dict(pool=pool,kind=kind,block=head["block"],selector=reader.tag,payer=OWNER,module=MODULE,executor=EXEC,
                runtime="0x"+keccak(hexstr=deployed).hex(),fills=fills)


if __name__=="__main__":
    cli=argparse.ArgumentParser(description=__doc__)
    cli.add_argument("kind",choices=MARKETS)
    cli.add_argument("artifact",type=Path)
    cli.add_argument("output",type=Path)
    cli.add_argument("--manifest",type=Path)
    args=cli.parse_args()
    args.output.write_text(json.dumps(capture(args.kind,json.loads(args.artifact.read_bytes()),args.manifest),separators=(",",":")),encoding="utf8")

