"""Replay measured Mooniswap legs with the guarded wallet and actual V5 step 17."""
import argparse
import gzip
import json
from pathlib import Path
from hashlib import sha256

from eth_abi import encode
from eth_utils import keccak
from fork_fee_rpc import Reader
from record_base_slipstream_executor import EXEC, OWNER, ROOT, RUNTIME, STEP, simulate_block
from record_bitswap_ethereum import tx
from record_mooniswap import FACTORIES, FUNDING, WETH, ZERO, read_balance

MODULE = "0x"+keccak(text="Mooniswap isolated guarded wallet Ethereum").hex()[-40:]
ORDER = "(bytes32,address,address,uint256,uint256,uint256)"


def runtime(reader, artifact, parameters):
    types = ["address","address","bool"]
    data = "0x"+artifact["creation"]+encode(types,parameters).hex()
    returned = reader.one("eth_call", [{"from":OWNER,"data":data},reader.tag])
    normalized = bytearray.fromhex(returned[2:])
    words = set()
    for refs in artifact["immutableReferences"].values():
        fields = {bytes(normalized[r["start"]:r["start"]+r["length"]]) for r in refs}
        if len(fields)!=1 or any(r["length"]!=32 for r in refs):
            raise ValueError("ambiguous immutable")
        words.update(fields)
        for r in refs:
            normalized[r["start"]:r["start"]+32]=bytes(32)
    if normalized.hex()!=artifact["runtime"] or words!={encode([t],[p]) for t,p in zip(types,parameters,strict=True)}:
        raise ValueError("unexpected constructor result")
    return returned


def capture(generation, artifact, manifest):
    folder = ROOT/"tests/data/first_run_dex_2026-11-26"
    original = json.loads(gzip.decompress((folder/f"mooniswap_{generation}_ethereum.json.gz").read_bytes()))
    head = json.loads((folder/"ethereum.json").read_bytes())
    reader = Reader("ethereum",head)
    if reader.tag!=original["selector"] or reader.header["stateRoot"]!=original["state_root"]:
        raise ValueError("registry parent")
    deadline = int(reader.header["timestamp"],16)+300
    factory = FACTORIES[generation][0]
    deployed = runtime(reader,artifact,[factory,WETH,generation=="successor"])
    word = lambda a: "0x"+format(int(a,16),"064x")
    states = {OWNER:{"balance":hex(100*10**18)},MODULE:{"code":deployed},
              EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
                    "stateDiff":{word("0"):word(OWNER),word("1"):word(WETH)}}}
    if any(reader.one("eth_getCode",[a,reader.tag])!="0x" for a in states):
        raise ValueError("simulation address occupied")
    markets=[]
    for market in original["markets"]:
        pool=market["pool"]
        cases=[]
        for leg in market["trials"]:
            if "exact" not in leg:
                continue
            a,b,n=leg["input"],leg["output"],int(leg["amount"])
            flags=1 if a==ZERO else 2 if b==ZERO else 0
            pool_id=((flags<<160)|int(pool,16)).to_bytes(32,"big")
            alternatives=[("wallet",a,b)]
            normalized=[WETH if x==ZERO else x for x in (a,b)]
            if normalized[0]!=normalized[1]:
                if ZERO in (a,b):
                    alternatives.append(("wallet_wrapped",*normalized))
                alternatives.append(("v5",*normalized))
            for mode,source,target in alternatives:
                limit=int(leg["exact"]["received"])
                endpoint=EXEC if mode=="v5" else MODULE
                # The experiment funds the signer with its own ETH, acquires ERC20 at the real independent
                # router and leaves authentic ETH/WETH inventory in the module before the measured trade.
                prefix=[dict(to=MODULE,data="0x",value="0x7",gas="0x186a0",**{"from":OWNER}),
                        tx(WETH,"deposit()",value=11),tx(WETH,"transfer(address,uint256)",["address","uint256"],[MODULE,11])]
                if source==WETH: prefix.append(tx(WETH,"deposit()",value=n))
                elif source!=ZERO:
                    prefix.append(tx(FUNDING,"swapETHForExactTokens(uint256,address[],address,uint256)",
                                     ["uint256","address[]","address","uint256"],[n,[WETH,source],OWNER,deadline],value=10**18))
                if source!=ZERO:
                    prefix.append(tx(source,"approve(address,uint256)",["address","uint256"],[endpoint,n]))
                marks=[read_balance(s,w) for w in (OWNER,pool) for s in ((source,target) if w==OWNER else (a,b))]
                marks.extend(read_balance(s,MODULE) for s in dict.fromkeys([source,target,ZERO,WETH]))
                def order(floor,end):
                    if mode=="v5":
                        step=(17,MODULE,source,target,10000,encode(["bytes32"],[pool_id]))
                        return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                                  ["address","uint256","address","uint256","uint256",STEP+"[]"],[source,n,target,floor,end,[step]])
                    return tx(MODULE,"execute("+ORDER+")",[ORDER],[(pool_id,source,target,n,floor,end)],value=n if source==ZERO else 0)
                trade=order(limit,deadline)
                cmds=prefix+marks+[order(limit+1,deadline),order(limit,0),trade]+marks
                rows=simulate_block(None,reader.tag,states,cmds,read_rpc=lambda _,method,p:reader.one(method,p))
                left=len(prefix)
                before=rows[left:left+len(marks)]
                bad,expired,filled=rows[left+len(marks):left+len(marks)+3]
                after=rows[-len(marks):]
                if any(r["status"]!="0x1" for r in rows[:left]+before+after) or [r["status"] for r in [bad,expired,filled]]!=["0x0","0x0","0x1"]:
                    raise ValueError(("payment/controls",generation,pool,source,target,n,mode,str(filled.get("error"))[:250]))
                old,new=([int(v["returnData"],16) for v in group] for group in (before,after))
                if (old[0]-new[0],new[1]-old[1],new[2]-old[2],old[3]-new[3])!=(n,limit,n,limit) or old[4:]!=new[4:]:
                    raise ValueError("wallet, pool or dust mismatch")
                topic="0x"+keccak(text="Paid(address,address,address,address,address,uint256,uint256)").hex()
                event,=[l for l in filled["logs"] if l["address"].lower()==MODULE and l["topics"][0]==topic]
                if int(event["topics"][3],16)!=int(pool,16):
                    raise ValueError("different paid pool")
                cases.append(dict(mode=mode,pool=pool,raw_pair=[a,b],input=source,output=target,amount=str(n),received=str(limit),
                                  bare_gas=leg["exact"]["gas"],gas=int(filled["gasUsed"],16),approvals_gas=0 if source==ZERO else int(rows[left-1]["gasUsed"],16),
                                  before=list(map(str,old)),after=list(map(str,new)),high=bad["error"],expired=expired["error"],trade=trade,logs=filled["logs"]))
        markets.extend(cases)
    if manifest is not None:
        if {case["mode"] for case in markets} != {"wallet", "wallet_wrapped", "v5"}:
            raise ValueError("unmeasured wallet forms")
        args = encode(["address","address","bool"], [factory,WETH,generation=="successor"])
        creation = bytes.fromhex(artifact["creation"]) + args
        roles = {}
        for name, address in (("factory", factory), ("wrapped", WETH)):
            code = reader.one("eth_getCode", [address, reader.tag])
            if not code or code == "0x":
                raise ValueError(("missing role", name))
            roles[name] = {"address":address,"keccak256":"0x"+keccak(hexstr=code).hex()}
        data = dict(kind="mooniswap_v2" if generation=="successor" else "mooniswap_v1",
                    chainId=1,network="Ethereum",roles=roles,hasSwapFor=generation=="successor",
                    sourceBlock=reader.tag["blockHash"],runtimeKeccak="0x"+keccak(hexstr=deployed).hex(),
                    runtimeSha256=sha256(bytes.fromhex(deployed[2:])).hexdigest(),creationSha256=sha256(creation).hexdigest(),
                    data="0x"+creation.hex(),solc=artifact["solc"])
        manifest.write_text(json.dumps(data,separators=(",",":")),encoding="utf8")
    return dict(generation=generation,factory=factory,selector=reader.tag,owner=OWNER,module=MODULE,executor=EXEC,
                module_hash="0x"+keccak(hexstr=deployed).hex(),cases=markets)


if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__);ap.add_argument("generation",choices=FACTORIES)
    ap.add_argument("artifact",type=Path);ap.add_argument("output",type=Path);ap.add_argument("--manifest",type=Path);opt=ap.parse_args()
    opt.output.write_text(json.dumps(capture(opt.generation,json.loads(opt.artifact.read_bytes()),opt.manifest),separators=(",",":")),encoding="utf8")
