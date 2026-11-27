"""Measure sender-funded LLAMMA wallet bounds independently of its public price getter."""
import argparse
import gzip
import json
from hashlib import sha256
from pathlib import Path

from eth_abi import decode, encode
from eth_utils import keccak
from record_base_slipstream_executor import OWNER, ROOT, simulate_block
from record_bitswap_ethereum import tx
from record_llamma_wallets import approve, base_funding
from record_mooniswap import Reader, WETH, read_balance

MODULE = "0x"+keccak(text="Ethereum LLAMMA isolated owner module").hex()[-40:]
ORDER = "(uint256,address,bytes32,address,address,uint256,uint256,uint256,bool)"
PAID = "0x"+keccak(text="Paid(address,address,address,address,uint256,uint256)").hex()
UINT = "uint256"


def runtime(reader, artifact, factory, tuples):
    creation = bytes.fromhex(artifact["creation"])+encode(["address","bool"],[factory,tuples])
    deployed = reader.one("eth_call",[{"from":OWNER,"data":"0x"+creation.hex()},reader.tag])
    code=bytearray.fromhex(deployed[2:])
    expected={encode(["address"],[factory]),encode(["bool"],[tuples]),keccak(hexstr=reader.one("eth_getCode",[factory,reader.tag]))}
    found=set()
    for refs in artifact["immutableReferences"].values():
        words={bytes(code[r["start"]:r["start"]+r["length"]]) for r in refs}
        if len(words)!=1 or any(r["length"]!=32 for r in refs):raise ValueError("ambiguous immutable")
        found.update(words)
        for r in refs:code[r["start"]:r["start"]+32]=bytes(32)
    if code.hex()!=artifact["runtime"] or expected!=found:raise ValueError("constructor returned different runtime")
    return deployed


def capture(artifact, manifest_dir):
    folder=ROOT/"tests/data/first_run_dex_2026-11-26"
    def archive(name):
        return json.loads(gzip.decompress((folder/("llamma_ethereum_"+name+".json.gz")).read_bytes()))
    bank=archive("registry")
    old=archive("wallets")
    reader=Reader("ethereum",json.loads((folder/"ethereum.json").read_bytes()))
    if reader.tag!=old["selector"] or reader.tag!=bank["selector"] or reader.header["stateRoot"]!=bank["root"]:
        raise ValueError("unrelated LLAMMA samples")
    if any(reader.one("eth_getCode",[a,reader.tag])!="0x" for a in (OWNER,MODULE)):
        raise ValueError("simulation address occupied")
    end=int(reader.header["timestamp"],16)+300
    modules={k:runtime(reader,artifact,f,k=="v2") for k,f in bank["factories"].items()}
    funded={}
    results=[]
    for row in old["rows"]:
        leg=row["result"]
        if row["mode"]=="v5" or leg["status"]!="paid":continue
        k,i,side=row["kind"],row["index"],row["side"]
        market=bank["markets"][k][i]
        amm=market["amm"]
        src,dst=[market["borrowed"],market["collateral"]][::1 if side==0 else -1]
        cap,spent,net=map(int,(leg["cap"],leg["spent"],leg["received"]))
        by_target=row["mode"]=="target"
        prefix=base_funding()
        key=(k,i,row["budget"])
        if row.get("funding")=="WETH.deposit":prefix=[tx(WETH,"deposit()",value=cap)]
        elif side:
            amount,buy=funded[key]
            prefix.extend([approve(dst,amm,amount),buy])
        elif not by_target:
            funded[key]=(cap,leg["trade"])
        prefix.append(approve(src,MODULE,cap))
        states={OWNER:{"balance":hex(100*10**18)},MODULE:{"code":modules[k]}}
        poolhash=keccak(hexstr=reader.one("eth_getCode",[amm,reader.tag]))
        def order(maximum,minimum,deadline):
            return tx(MODULE,"execute("+ORDER+")",[ORDER],[(i,amm,poolhash,src,dst,maximum,minimum,deadline,by_target)])
        marks=[read_balance(t,a) for a in (OWNER,amm,MODULE) for t in (src,dst)]
        marks.append(tx(src,"allowance(address,address)",["address","address"],[MODULE,amm]))
        trade=order(cap,net,end)
        bound=order(spent-1,net,end) if by_target else order(cap,net+1,end)
        cmds=prefix+marks+[bound,order(cap,net,0),trade]+marks
        rows=simulate_block(None,reader.tag,states,cmds,read_rpc=lambda _,m,p:reader.one(m,p))
        pos=len(prefix)
        if any(r["status"]!="0x1" for r in rows[:pos+len(marks)]+rows[-len(marks):]):raise ValueError(("funding",k,i,side))
        high,late,paid=rows[pos+len(marks):pos+len(marks)+3]
        if [x["status"] for x in (high,late,paid)]!=["0x0","0x0","0x1"]:raise ValueError(("payment",k,i,side,by_target,paid.get("error")))
        before=[int(r["returnData"],16) for r in rows[pos:pos+len(marks)]]
        after=[int(r["returnData"],16) for r in rows[-len(marks):]]
        if [b-a for a,b in zip(before,after,strict=True)]!=[-spent,net,spent,-net,0,0,0] or before[-1]:
            raise ValueError("physical balance or remaining allowance")
        event,=[l for l in paid["logs"] if l["address"].lower()==MODULE and l["topics"][0]==PAID]
        if [int(t,16) for t in event["topics"][1:]]!=[int(a,16) for a in (OWNER,amm,src)] or decode(["address",UINT,UINT],bytes.fromhex(event["data"][2:]))!=(dst,spent,net):
            raise ValueError("module event mismatch")
        results.append(dict(kind=k,index=i,side=side,mode=row["mode"],funding=row.get("funding","USDT/crvUSD"),cap=str(cap),spent=str(spent),received=str(net),
                            before=list(map(str,before)),after=list(map(str,after)),gas=int(paid["gasUsed"],16),approvalGas=int(rows[pos-1]["gasUsed"],16),
                            bareGas=leg["gas"],control=[high["error"],late["error"]],trade=trade,logs=paid["logs"]))
    for key,code in modules.items():
        if {r["side"] for r in results if r["kind"]==key}!={0,1}:
            raise ValueError("missing paid side")
        if manifest_dir:
            factory=bank["factories"][key]
            creation=bytes.fromhex(artifact["creation"])+encode(["address","bool"],[factory,key=="v2"])
            kind="llamma_lending" if key=="oneway" else "llamma_"+key
            manifest=dict(kind=kind,chainId=1,network="Ethereum",tupleRegistry=key=="v2",roles={
                "factory":{"address":factory,"keccak256":bank["runtimes"][key]["factoryKeccak"]}},
                sourceBlock=reader.tag["blockHash"],runtimeKeccak="0x"+keccak(hexstr=code).hex(),
                runtimeSha256=sha256(bytes.fromhex(code[2:])).hexdigest(),creationSha256=sha256(creation).hexdigest(),
                data="0x"+creation.hex(),solc=artifact["solc"])
            (manifest_dir/("llamma-"+key+"-create.json")).write_text(json.dumps(manifest,separators=(",",":")),encoding="utf8")
    return dict(selector=reader.tag,owner=OWNER,module=MODULE,runtimes={k:"0x"+keccak(hexstr=c).hex() for k,c in modules.items()},rows=results)


if __name__=="__main__":
    cli=argparse.ArgumentParser(description=__doc__);cli.add_argument("artifact",type=Path);cli.add_argument("output",type=Path)
    cli.add_argument("--manifest-dir",type=Path);opt=cli.parse_args();opt.output.write_text(json.dumps(capture(json.loads(opt.artifact.read_bytes()),opt.manifest_dir),separators=(",",":")),encoding="utf-8")

