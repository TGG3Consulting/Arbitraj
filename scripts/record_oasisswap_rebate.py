"""Check optional rebates using fresh, independently owned contracts in one read-only simulation."""
import argparse
import json
from pathlib import Path

from eth_abi import decode, encode
from eth_utils import keccak
import requests

from record_base_slipstream_executor import EXEC, RUNTIME, STEP, rpc, simulate_block
from record_oasisswap_v5 import FACTORY, ROUTER, WETH, allowance, balance, scalar, tx, word


def isolated(label):
    return "0x" + keccak(text="isolated Oasis rebate regression: " + label)[-20:].hex()


PAYER = isolated("payer")
TOKENS = sorted([isolated("first wrapped asset"), isolated("second wrapped asset")])
ESTIMATOR = isolated("optional estimator")
RESERVE = 10**18
SIZE = 10**16


def created(nonce):
    # RLP([20-byte sender, nonce]); only the four initial, single-byte nonces are used.
    return "0x" + keccak(bytes.fromhex("d694" + PAYER[2:] + ("80" if nonce == 0 else f"{nonce:02x}")))[-20:].hex()


def source(endpoint, directory, address):
    url = directory["origin"][address]["url"]
    reply = requests.get(url, params={"fields":"creationBytecode.onchainBytecode,runtimeBytecode.onchainBytecode"}, timeout=30)
    reply.raise_for_status()
    doc = reply.json()
    live = rpc(endpoint, "eth_getCode", [address, directory["selector"]])
    if live.lower() != doc["runtimeBytecode"]["onchainBytecode"].lower() or "0x"+keccak(hexstr=live).hex() != directory["origin"][address]["runtime"]:
        raise ValueError("verified deployment source disagrees with the accepted header")
    return doc["creationBytecode"]["onchainBytecode"]


def replay(endpoint, directory, artifact):
    tag = directory["selector"]
    head = rpc(endpoint, "eth_getBlockByHash", [tag["blockHash"], False])
    if rpc(endpoint, "eth_chainId", []) != "0x2105" or head["stateRoot"] != directory["state_root"]:
        raise ValueError("wrong archive")
    factory, module, other, native = [created(i) for i in range(4)]
    a, b = TOKENS
    init = directory["init"]
    salt = keccak(bytes.fromhex(a[2:]+b[2:]))
    pool = "0x"+keccak(b"\xff"+bytes.fromhex(factory[2:])+salt+bytes.fromhex(init[2:]))[-20:].hex()
    empty = [PAYER, a, b, ESTIMATOR, factory, module, other, native, pool]
    if any(rpc(endpoint, "eth_getCode", [address, tag]) != "0x" for address in empty):
        raise ValueError("isolated address collides with deployed code")
    fc = source(endpoint, directory, FACTORY)
    router = source(endpoint, directory, ROUTER)
    args = encode(["address","address"], [FACTORY,WETH]).hex()
    if not router.endswith(args):
        raise ValueError("official router constructor parameters differ")
    def create(data):
        return {"data":data, "gas":hex(6_000_000)}
    def addr(x):
        return encode(["address"],[x]).hex()
    setup = [create(fc), create("0x"+artifact["creation"]+addr(factory)),
             create("0x"+artifact["creation"]+addr(FACTORY)), create(router[:-len(args)]+addr(factory)+addr(a)),
             tx(factory,"setRebateEstimator(address)",["address"],[ESTIMATOR]),
             {**tx(factory,"createPair(address,address)",["address","address"],[a,b]),"gas":hex(5_000_000)}]
    for token in TOKENS:
        setup.extend([tx(token,"deposit()",value=RESERVE+SIZE),
                      tx(token,"transfer(address,uint256)",["address","uint256"],[pool,RESERVE])])
    setup.extend([tx(pool,"mint(address)",["address"],[PAYER]),
                  tx(a,"approve(address,uint256)",["address","uint256"],[EXEC,SIZE])])
    wrapped = rpc(endpoint, "eth_getCode", [WETH, tag])
    overrides = {PAYER:{"nonce":"0x0","balance":hex(10**20)}, a:{"code":wrapped}, b:{"code":wrapped},
                 EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
                       "stateDiff":{word(0):word(int(PAYER,16)),word(1):word(int(a,16))}}}
    deadline = int(head["timestamp"],16) + 120
    def trade(adapter, floor):
        return tx(EXEC, "execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                  ["address","uint256","address","uint256","uint256",STEP+"[]"],
                  [a,SIZE,b,floor,deadline,[(29,adapter,a,b,10000,b"")]])
    cache = tx(pool, "feeCache0()")
    reads = [balance(a,PAYER),balance(b,PAYER),allowance(a,PAYER,EXEC),cache,
             balance(a,EXEC),balance(b,EXEC),allowance(a,EXEC,module),balance(a,module),balance(b,module)]
    def quote(rebated):
        return tx(native,"getAmountsOut(uint256,address[],bool,address)",["uint256","address[]","bool","address"],
                  [SIZE,[a,b],rebated,PAYER])
    cases=[]
    for value in (0,1000,10000,10001):
        fee = 100 if value>10000 else 100*(10000-value)//10000
        weighted = SIZE*(10000-fee)
        expected = weighted*RESERVE//(RESERVE*10000+weighted)
        # getRebate(address) returns value for PAYER and zero for any other address.
        # Only this generated oracle has an injected implementation. Production participants are never impersonated.
        overrides[ESTIMATOR] = {"code":"0x60043573"+PAYER[2:]+"1461"+format(value,"04x")+"0260005260206000f3"}
        controls = [tx(factory,"owner()"),tx(factory,"getPair(address,address)",["address","address"],[a,b]),
                    tx(module,"factory()"),tx(other,"factory()"),tx(module,"MAX_REBATE()"),
                    tx(factory,"getRebate(address)",["address"],[PAYER]),tx(factory,"getRebate(address)",["address"],[EXEC]),
                    tx(pool,"calculateFee(uint64)",["uint64"],[value]),quote(False),quote(True)]
        calls=setup+controls+reads+[trade(other,1),trade(module,expected+1)]+reads+[trade(module,expected)]+reads
        replies=simulate_block(endpoint,tag,overrides,[{**c,"from":PAYER} for c in calls])
        if any(x["status"]!="0x1" for x in replies[:len(setup)]):
            i=next(i for i,x in enumerate(replies[:len(setup)]) if x["status"]!="0x1")
            raise ValueError(f"fresh contract setup step {i}: {replies[i].get('error')}")
        i=len(setup); check=replies[i:i+len(controls)]; i+=len(controls)
        if [scalar(x) for x in check[:7]] != [int(PAYER,16),int(pool,16),int(factory,16),int(FACTORY,16),10000,value,0]:
            raise ValueError("factory, payer, recipient, pair or module binding differs")
        def amount(x):
            if x["status"]!="0x1": return None
            result,=decode(["uint256[]"],bytes.fromhex(x["returnData"][2:]))
            if len(result)!=2 or result[0]!=SIZE: raise ValueError("router returned another path")
            return result[1]
        nominal=SIZE*9900*RESERVE//(RESERVE*10000+SIZE*9900)
        if amount(check[8])!=nominal or amount(check[9])!=(None if value>10000 else expected):
            raise ValueError("official router nominal/discounted results differ")
        if value>10000:
            if check[7]["status"]!="0x0": raise ValueError("official pair accepted an invalid fee")
        elif scalar(check[7])!=fee: raise ValueError("official pair fee differs")
        n=len(reads)
        before=list(map(scalar,replies[i:i+n])); i+=n
        wrong,strict=replies[i:i+2]; i+=2
        rolled=list(map(scalar,replies[i:i+n])); i+=n
        accepted=replies[i]; after=list(map(scalar,replies[i+1:]))
        if wrong["status"]!="0x0" or strict["status"]!="0x0" or rolled!=before:
            raise ValueError("wrong factory or minimum did not roll back completely")
        taken=SIZE*fee*5000//10000//10000
        if (accepted["status"],before[0]-after[0],after[1]-before[1],after[2],after[3]-before[3],any(after[4:])) != ("0x1",SIZE,expected,0,taken,False):
            raise ValueError("payer balance, fee cache, allowance or module residue differs")
        cases.append(dict(rebate=value,recipient_rebate=0,fee=fee,nominal=str(nominal),received=str(expected),
                          native_discounted=None if value>10000 else str(expected),fee_cache_delta=str(taken),
                          min_plus_one=strict["status"],wrong_factory=wrong["status"],gas=int(accepted["gasUsed"],16)))
    return dict(scope="Fresh isolated contracts; verified factory/pair and router code. Hypothetical estimator settings.",
                selector=tag,state_root=head["stateRoot"],source=directory["origin"],factory=factory,native=native,pool=pool,
                payer=PAYER,recipient_in_adapter=EXEC,amount=str(SIZE),reserve=str(RESERVE),cases=cases)


if __name__ == "__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    for name in ("directory","artifact","output"): parser.add_argument(name,type=Path)
    args=parser.parse_args()
    for endpoint in ("https://mainnet.base.org","https://base.gateway.tenderly.co","https://base-rpc.publicnode.com"):
        try:
            data=replay(endpoint,json.loads(args.directory.read_bytes()),json.loads(args.artifact.read_bytes()))
            args.output.write_text(json.dumps(data,indent=2)+"\n",encoding="utf8")
            break
        except (ValueError, OSError, KeyError, RuntimeError) as e:
            print(endpoint,str(e)[:400],flush=True)
    else: raise ValueError("no canonical endpoint completed the optional rebate regression")
