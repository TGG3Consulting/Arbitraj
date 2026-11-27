"""Выплата через CarbonWallet на каноническом родителе официальных контролей, без ключа или отправки транзакций."""
from record_carbon_ethereum import ETH_MARKET
import argparse
import json
from hashlib import sha256
from pathlib import Path
import requests
from eth_abi import encode, decode
from eth_utils import keccak
from record_carbon_ethereum import ACTION, BLOCK, CTRL, ENDPOINTS, FUNDING, MULTI, NATIVE, OWNER, WETH, rpc, simulate_block, tx

MODULE = "0x7900000000000000000000000000000000000064"
ABI = ["address", "address", ACTION, "bool", "uint256", "uint128"]


def runtime_at(url, tag, artifact, controller=CTRL,session=requests):
    word = encode(["address"], [controller])
    expected = bytearray.fromhex(artifact["runtime"])
    refs = list(artifact["immutableReferences"].values())
    if len(refs) != 1 or not refs[0]:
        raise ValueError("expected one immutable controller")
    for ref in refs[0]:
        start = ref["start"]
        if ref["length"] != len(word):
            raise ValueError("incorrect immutable width")
        expected[start:start + 32] = word
    creation = {"from": OWNER, "data": "0x" + artifact["creation"] + word.hex()}
    reply = session.post(url, json={"jsonrpc":"2.0","id":1,"method":"eth_call","params":[creation,tag]},timeout=35)
    reply.raise_for_status()
    if reply.json().get("result") != "0x"+expected.hex() or rpc(url,"eth_getCode",[MODULE,tag]) != "0x":
        raise ValueError("bound creation/runtime or temporary module address")
    return "0x"+expected.hex()


def capture(url, source, artifact, market=ETH_MARKET, chain_id=1, block=BLOCK,session=requests):
    tag = source["selector"]
    if tag != {"blockHash":block,"requireCanonical":True} or int(rpc(url,"eth_chainId",[]),16) != chain_id:
        raise ValueError("incorrect physical network")
    header = rpc(url,"eth_getBlockByHash",[block,False])
    if header["stateRoot"] != source["state_root"]:
        raise ValueError("canonical state root")
    runtime = runtime_at(url,tag,artifact,market.controller,session)
    overrides = {OWNER:{"balance":hex(20*10**18)}, MODULE:{"code":runtime}}
    rows=[]
    for v in source["controls"]:
        original = v["transaction"]
        src,dst,actions,deadline,_=decode(["address","address",ACTION,"uint256","uint128"],bytes.fromhex(original["data"][10:]))
        spent, received, by_target=int(v["input"]),int(v["output"]),v["by_target"]
        if src!=v["source"] or dst!=v["target"] or original["to"]!=market.controller or original["from"]!=OWNER:
            raise ValueError("original signed market")
        def order(limit, expires):
            value=(limit if by_target else spent) if src==NATIVE else 0
            return tx(MODULE,"execute(address,address,"+ACTION+",bool,uint256,uint128)",ABI,[src,dst,actions,by_target,expires,limit],value)
        def balance(a,w):
            return tx(MULTI,"getEthBalance(address)",["address"],[w]) if a==NATIVE else tx(a,"balanceOf(address)",["address"],[w])
        prep=[]
        if src!=NATIVE:
            prep=[tx(market.funding,"swapETHForExactTokens(uint256,address[],address,uint256)",["uint256","address[]","address","uint256"],[spent,[market.wrapped,src],OWNER,deadline],10**18),
                  tx(src,"approve(address,uint256)",["address","uint256"],[MODULE,spent])]
        # Запас при точном выходе проверяет возврат собственных ETH отправителю; сравниваются обе стороны и остаток модуля.
        limit=spent+(1 if src==NATIVE else 0) if by_target else received
        balances=[balance(a,w) for w in [OWNER,market.controller,MODULE] for a in [src,dst]]
        calls=prep+balances+[order(spent-1 if by_target else received+1,deadline),order(limit,1)]+balances+[order(limit,deadline)]+balances
        result=simulate_block(url,tag,overrides,calls)
        n=len(prep)
        if [j for j,x in enumerate(result) if x["status"]!="0x1"] != [n+6,n+7]:
            raise ValueError(("unexpected outcomes",src,dst,by_target,[(j,x.get("error")) for j,x in enumerate(result) if x["status"]!="0x1"]))
        def balances_at(i):
            return [int(a["returnData"],16) for a in result[i:i+6]]
        a,b,c=balances_at(n),balances_at(n+8),balances_at(n+15)
        if a!=b or [a[0]-c[0],c[1]-a[1],c[2]-a[2],a[3]-c[3]]!=[spent,received,spent,received] or a[4:]!=c[4:]:
            raise ValueError("wallet, controller or temporary module balance")
        fill=result[n+14]
        rows.append(dict(source=src,target=dst,owner=OWNER,by_target=by_target,actions=[[str(i),str(q)] for i,q in actions],
                         input=str(spent),output=str(received),before=list(map(str,a)),after=list(map(str,c)),
                         transaction=calls[n+14],logs=fill["logs"],gas=int(fill["gasUsed"],16),
                         exact_refusal=result[n+6]["error"],expired=result[n+7]["error"]))
    creation = bytes.fromhex(artifact["creation"])+encode(["address"],[market.controller])
    roles=source["roles"]
    if isinstance(roles,list):
        implementation=roles[1]
        factory_hash,implementation_hash=source["hashes"]
    else:
        implementation=roles["implementation"]["address"]
        factory_hash=roles["proxy"]["keccak256"]
        implementation_hash=roles["implementation"]["keccak256"]
    manifest=dict(kind="carbon_v1",chainId=chain_id,market=market.controller,implementation=implementation,
                  factoryHash=factory_hash,implementationHash=implementation_hash,sourceBlock=tag["blockHash"],
                  data="0x"+creation.hex(),creationSha256=sha256(creation).hexdigest(),
                  runtimeSha256=sha256(bytes.fromhex(runtime[2:])).hexdigest(),runtimeKeccak="0x"+keccak(hexstr=runtime).hex(),solc=artifact["solc"])
    return dict(selector=tag,state_root=source["state_root"],controller=market.controller,module=MODULE,manifest=manifest,
                runtime_hash="0x"+keccak(hexstr=runtime).hex(),controls=rows)


if __name__=="__main__":
    parser=argparse.ArgumentParser()
    for arg in ["source","artifact","output"]:
        parser.add_argument(arg,type=Path)
    args=parser.parse_args()
    errors=[]
    for url in ENDPOINTS:
        try:
            out=capture(url,json.loads(args.source.read_text()),json.loads(args.artifact.read_text()))
            args.output.write_text(json.dumps(out,separators=(",",":"))+"\n");break
        except (ValueError, KeyError, RuntimeError, requests.RequestException) as exc:
            errors.append((url,str(exc)[:450]))
    else:
        raise RuntimeError(errors)
