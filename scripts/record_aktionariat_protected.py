"""Compare direct and V5 settlements against measured wallet cash on the source Ethereum block."""
import argparse
import gzip
import hashlib
import json
from pathlib import Path

import requests
from eth_abi import encode
from eth_utils import keccak

from record_aktionariat_wallet import DIRECTORY, FUND, NATIVE_ROUTER, SWAP, UINT, WETH, approve, verdict
from record_balancer_markets import Capture
from record_base_slipstream_executor import EXEC, OWNER, ROOT, RUNTIME, STEP, rpc, simulate_block
from record_bitswap_ethereum import balance, tx

MODULE = "0x" + keccak(bytes.fromhex("d694"+OWNER[2:]+"80"))[-20:].hex()  # CREATE from nonce 0
HUB = "0xa537d23a76ec454f0874ad4508794b17ed9be610"
ORDER = "(address,address,address,uint256,uint256,uint256)"
BASELINE = DIRECTORY.with_name("aktionariat_ethereum_native.json.gz")


def capture(url, compiled, directory, native):
    tag = native["selector"]
    assert tag == directory["selector"] and native["block"] == directory["height"]
    header = rpc(url,"eth_getBlockByHash",[tag["blockHash"],False])
    assert header["stateRoot"] == directory["state_root"] and rpc(url,"eth_chainId",[]) == "0x1"
    registry = native["registry"]
    assert all(rpc(url,"eth_getCode",[a,tag]) == "0x" for a in (EXEC,MODULE,OWNER))
    c = Capture(dict(selector=tag,endpoint=url))
    assert rpc(url,"eth_getTransactionCount",[OWNER,tag]) == "0x0"
    issuers=sorted(m["address"] for m in directory["markets"].values() if m["registered_pair"] is None)
    creation=bytes.fromhex(compiled["creation"])+encode(["address","address","address[]"],[registry,HUB,issuers])
    deploy={"from":OWNER,"data":"0x"+creation.hex(),"gas":hex(6000000)}
    response=c.session.post(url,json={"jsonrpc":"2.0","id":1,"method":"eth_call","params":[deploy,tag]},timeout=30)
    response.raise_for_status()
    code=response.json()["result"]
    normalized=bytearray.fromhex(code[2:])
    bindings=set()
    for positions in compiled["immutableReferences"].values():
        group={bytes(normalized[p["start"]:p["start"]+p["length"]]) for p in positions}
        assert len(group)==1 and all(p["length"]==32 for p in positions)
        bindings.update(group)
        for p in positions: normalized[p["start"]:p["start"]+32]=bytes(32)
    assert normalized==bytes.fromhex(compiled["runtime"])
    assert bindings=={encode(["address"],[a]) for a in (registry,HUB)}
    deadline = int(header["timestamp"],16)+900
    word = lambda x: "0x"+format(x,"064x")
    env = {OWNER:{"balance":hex(100*10**18)}, 
           EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
                 "stateDiff":{word(0):word(int(OWNER,16)),word(1):word(int(WETH,16))}}}

    validation=simulate_block(url,tag,env,[deploy,tx(MODULE,"registry()"),tx(MODULE,"issuerHub()"),
          *[tx(MODULE,"issuerMarkets(address)",["address"],[a]) for a in [*issuers,OWNER]]])
    assert all(v["status"]=="0x1" for v in validation)
    assert [int(v["returnData"],16) for v in validation[1:]]==[int(registry,16),int(HUB,16),*[1]*len(issuers),0]
    def order(mode, broker, source, target, budget, floor, end):
        if mode == "wallet":
            return tx(MODULE,"execute("+ORDER+")",[ORDER],[(broker,source,target,budget,floor,end)])
        leg = (17,MODULE,source,target,10000,encode(["uint256"],[(budget<<160)|int(broker,16)]))
        return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                  ["address",UINT,"address",UINT,UINT,STEP+"[]"],[source,budget,target,floor,end,[leg]])

    controls = []
    baseline = {}
    for row in native["controls"]:
        if row.get("buy_received"):
            baseline.setdefault((row["market"],row["units"]), row)
    for (broker,units), row in baseline.items():
        market=directory["markets"][broker]
        cash, share=market["cash"],market["share"]
        cost=int(row["buy_spent"])
        budget=10*10**18
        funding=[tx(WETH,"deposit()",value=budget),approve(WETH,FUND,budget),
                 tx(FUND,"exactOutputSingle("+SWAP+")",[SWAP],[(WETH,cash,3000,OWNER,deadline,cost+1,budget,0)])]
        if row["mode"] == "hub":
            native_buy=[approve(cash,market["hub"],cost),tx(market["hub"],"payAndNotify(address,uint256,bytes)",
                        ["address",UINT,"bytes"],[broker,cost,b""])]
        else:
            native_buy=[approve(cash,NATIVE_ROUTER,cost),tx(NATIVE_ROUTER,"exactOutputSingle("+SWAP+")",[SWAP],
                        [(cash,share,0,OWNER,deadline,units,cost,0)])]
        for mode in ("wallet","v5"):
            payer=MODULE if mode=="wallet" else EXEC
            for buying in (True,False):
                source,target=(cash,share) if buying else (share,cash)
                amount=cost+1 if buying else units
                prep=[deploy,*funding,*([] if buying else native_buy),approve(source,payer,amount)]
                accounts=[balance(token,who) for who in (OWNER,MODULE,EXEC) for token in (source,target)]
                seq=[*prep,*accounts,order(mode,broker,source,target,amount,1,deadline),*accounts]
                result=simulate_block(url,tag,env,seq)
                cut=len(prep)
                assert all(v["status"]=="0x1" for v in result[:cut+6])
                before=[int(v["returnData"],16) for v in result[cut:cut+6]]
                after=[int(v["returnData"],16) for v in result[-6:]]
                payment=result[cut+6]
                evidence=dict(mode=mode,broker=broker,source=source,target=target,buying=buying,units=units,
                              budget=str(amount),before=list(map(str,before)),after=list(map(str,after)),payment=verdict(payment))
                controls.append(evidence)
                if payment["status"]!="0x1":
                    assert before==after
                    continue
                spent=before[0]-after[0];received=after[1]-before[1]
                assert 0<spent<=amount and received>0 and before[2:]==after[2:]
                assert int(payment["returnData"],16)==received
                if buying:
                    assert cost<=spent<=amount and received==units
                else:
                    assert spent==units
                probes=[order(mode,broker,source,target,amount,received+1,deadline),
                        order(mode,broker,source,target,amount,received,1),
                        order(mode,broker,source,target,amount,received,deadline)]
                checked=simulate_block(url,tag,env,[*prep,*accounts,*probes[:2],*accounts,probes[2],*accounts])
                assert all(v["status"]=="0x1" for v in checked[:cut+6])
                assert all(v["status"]=="0x0" for v in checked[cut+6:cut+8])
                assert before==[int(v["returnData"],16) for v in checked[cut+8:cut+14]]
                filled=checked[cut+14]
                assert filled["status"]=="0x1" and after==[int(v["returnData"],16) for v in checked[-6:]]
                evidence.update(spent=str(spent),received=str(received),request=probes[2],logs=filled["logs"],
                                refusal=[v["error"] for v in checked[cut+6:cut+8]],gas=int(filled["gasUsed"],16))
    assert any(x.get("received") for x in controls)
    manifest=dict(kind="aktionariat",chainId=1,sourceBlock=tag["blockHash"],registry=registry,hub=HUB,issuerMarkets=issuers,creationGas=int(validation[0]['gasUsed'],16),
                  registryHash="0x"+keccak(hexstr=rpc(url,"eth_getCode",[registry,tag])).hex(),
                  hubHash=directory["roles"][HUB]["runtime"],creationSha256=hashlib.sha256(creation).hexdigest(),
                  runtimeKeccak="0x"+keccak(hexstr=code).hex(),data="0x"+creation.hex(),solc=compiled["solc"])
    return dict(selector=tag,wallet=OWNER,module=MODULE,executor=EXEC,controls=controls,manifest=manifest)


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("compiled",type=Path)
    parser.add_argument("output",type=Path)
    args=parser.parse_args()
    with gzip.open(DIRECTORY) as f: directory=json.load(f)
    with gzip.open(BASELINE) as f: baseline=json.load(f)
    errors=[]
    for url in ("https://rpc.mevblocker.io","https://eth.drpc.org"):
        try:
            data=capture(url,json.loads(args.compiled.read_bytes()),directory,baseline)
            break
        except (requests.RequestException,ValueError,RuntimeError) as error:
            errors.append((url,str(error)[:250]))
    else:
        raise RuntimeError(errors)
    args.output.write_bytes(json.dumps(data,separators=(",",":")).encode())
