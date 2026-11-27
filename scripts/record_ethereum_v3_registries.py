"""Register Blueprint and VVS V3 Ethereum markets against archived factory events, contract creation count and executable roles."""
import argparse
import json
from pathlib import Path

import requests
from eth_abi import decode, encode
from eth_utils import keccak

from record_base_slipstream_executor import ROOT, calldata
from record_ethereum_x7 import ROOT_HASH

FACTORIES = {
    "shibaswap-v2": {
        "factory": "0xd9ce49caf7299daf18fffcb2b84a44fd33412509",
        "router": "0xb2ecc25c0b3af0039d4d9dddfcec19e958618963",
        "quoter": "0x486dd4ff6abd5b2f728192cda291d2ffb611cbd1",
        "lens": "0xbfd8137f7d1516d3ea5ca83523914859ec47f573",
    },
    "blueprint": {
        "factory": "0xe777c3da43ec554ec845649323215afaa34d6c23",
        "router": "0x269ae88aa04e7b9b81f2d89263fbcd2cef1c58fc",
        "quoter": "0x4c78d1ad9895125cd6a693a5b3aeae3c9478af0d",
        "lens": "0xbfd8137f7d1516d3ea5ca83523914859ec47f573",
    },
    "vvs": {
        "factory": "0x5e12f3bdeb62c6296fa457b1a69438d7fe8c6e2e",
        "deployer": "0x3c1afec311e71cb1fd55ea5fe76558731c5cde8a",
        "router": "0xe3df4f9b1454ec806a712932d62810ddc641b8cc",
        "quoter": "0x002212b3692ffcde4733a35a9fb4b61d07960bc9",
        "lens": "0x24bbfa1c7319d6da4e556fa77d6c530d87dfcf88",
    },
}
WETH = "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"
MULTICALL = "0xca11bde05977b3631167028862be2a173976ca11"
NAMES = ["PoolCreated(address,address,uint24,int24,address)", "FeeAmountEnabled(uint24,int24)"]
TOPICS = ["0x" + keccak(text=s).hex() for s in NAMES]


def capture(name, endpoint):
    addresses = FACTORIES[name]
    factory = addresses["factory"]
    deployer = addresses.get("deployer", factory)
    snapshot = json.loads((ROOT / "tests/data/first_run_dex_2026-11-26/ethereum.json").read_bytes())
    tag = snapshot["selector"]
    session = requests.Session()

    providers = list(dict.fromkeys([endpoint,"https://eth.drpc.org","https://ethereum-rpc.publicnode.com"]))
    verified = set()

    def ask(url, method, params):
        response = session.post(url, json=dict(jsonrpc="2.0",id=1,method=method,params=params), timeout=20)
        response.raise_for_status()
        body = response.json()
        if "result" not in body: raise ValueError((method,str(body.get("error"))[:120]))
        return body["result"]

    def rpc(method, params):
        errors=[]
        for url in tuple(providers):
            try:
                if url not in verified:
                    h=ask(url,"eth_getBlockByHash",[tag["blockHash"],False])
                    if not h or h["stateRoot"]!=ROOT_HASH or int(h["number"],16)!=snapshot["block"] or ask(url,"eth_chainId",[])!="0x1":
                        raise ValueError("unexpected provider header")
                    verified.add(url)
                reply=ask(url,method,params)
                if method!="eth_getLogs":
                    providers.remove(url);providers.insert(0,url)
                return reply
            except (ValueError,requests.RequestException) as error: errors.append(str(error)[:130])
        raise ValueError((method,errors))

    def views(calls):
        values = []
        for i in range(0, len(calls), 40):
            part = [(a, False, bytes.fromhex(calldata(sig, types, args)[2:])) for a, sig, types, args in calls[i:i+40]]
            data = calldata("aggregate3((address,bool,bytes)[])", ["(address,bool,bytes)[]"], [part])
            raw = rpc("eth_call", [{"to": MULTICALL, "data":data}, tag])
            returned, = decode(["(bool,bytes)[]"], bytes.fromhex(raw[2:]))
            if len(returned) != len(part) or any(not ok for ok, _ in returned):
                raise ValueError("incomplete canonical reads")
            values.extend(v for _, v in returned)
        return values

    head = rpc("eth_getBlockByHash", [tag["blockHash"], False])
    if int(head["number"],16) != snapshot["block"] or head["stateRoot"] != ROOT_HASH or rpc("eth_chainId", []) != "0x1":
        raise ValueError("different Ethereum header")
    roles = {}
    for role, address in addresses.items():
        url = "https://sourcify.dev/server/v2/contract/1/"+address
        source = session.get(url, params={"fields":"compilation,deployment,sources,runtimeBytecode.onchainBytecode"}, timeout=24)
        source.raise_for_status()
        provenance = source.json()
        bytecode = rpc("eth_getCode", [address, tag])
        if bytecode == "0x" or bytecode != provenance["runtimeBytecode"]["onchainBytecode"].lower() or provenance.get("runtimeMatch") not in ("match","exact_match"):
            raise ValueError(("runtime differs from source",role))
        roles[role] = dict(address=address,name=provenance["compilation"]["name"],runtime="0x"+keccak(hexstr=bytecode).hex(),
                           created=int(provenance["deployment"]["blockNumber"]),source=url)
        if role in ("quoter","router"):
            import re
            code = "\n".join(f["content"] for f in provenance["sources"].values())
            constants = set(re.findall(r"POOL_INIT_CODE_HASH\s*=\s*(?:0x|hex['\"])([a-fA-F0-9]{64})",code))
            if len(constants)!=1:raise ValueError(("periphery creation hash",role,constants))
            roles[role]["init"]="0x"+constants.pop().lower()
    init = roles["router"]["init"]
    if init != roles["quoter"]["init"]:
        raise ValueError("router and quoter disagree about pool creation")
    bound = []
    for role in ("router","quoter"):
        bound.extend([(addresses[role], "factory()", [], [], factory),(addresses[role],"WETH9()",[],[],WETH)])
        if name=="vvs":bound.append((addresses[role],"deployer()",[],[],deployer))
    if name=="vvs":bound.append((factory,"poolDeployer()",[],[],deployer))
    for value,(_,_,_,_,expected) in zip(views([r[:4] for r in bound]),bound,strict=True):
        if decode(["address"],value)[0]!=expected: raise ValueError(("wrong getter",expected))

    if name == "shibaswap-v2":
        creation,collector = views([(factory,"poolPairHash()",[],[]), (factory,"feeCollector()",[],[])])
        assert "0x"+creation.hex() == init
        collector, = decode(["address"],collector)
    tiers={}
    if name == "shibaswap-v2":
        from fork_fee_rpc import Reader, pool_blocks
        from record_v3_fee_registry import candidate_fees
        reader = Reader("ethereum",snapshot)
        created = roles["factory"]["created"]
        total = int(reader.one("eth_getTransactionCount",[factory,tag]),16)
        changed = pool_blocks(reader,factory,created-1,snapshot["block"],0,total)
        headers = reader.many(("eth_getBlockByNumber",[hex(h),False]) for h in changed)
        batches = reader.many(("eth_getLogs",[dict(address=factory,blockHash=h["hash"],topics=[TOPICS])]) for h in headers)
        logs = []
        for (height,difference),header,events in zip(changed.items(),headers,batches,strict=True):
            assert int(header["number"],16)==height
            assert sum(row["topics"][0]==TOPICS[0] for row in events) == difference-int(height==created)
            assert all(row["blockHash"]==header["hash"] and not row.get("removed") for row in events)
            logs.extend(events)
        fixtures = ROOT / "tests/data/first_run_dex_2026-11-26"
        fees = candidate_fees(json.loads((ROOT/"config/first_run_dex.json").read_bytes()),fixtures)
        fees.update(int(row["topics"][3],16) for row in logs if row["topics"][0]==TOPICS[0])
        fees = sorted(fees)
        entries = views([(factory,"feeAmountTickSpacing(uint24)",["uint24"],[fee]) for fee in fees])
        tiers = {fee:spacing for fee,data in zip(fees,entries,strict=True) if (spacing:=decode(["int24"],data)[0])>0}
    else:
        logs=rpc("eth_getLogs",[dict(address=factory,fromBlock=hex(roles["factory"]["created"]),toBlock=head["number"],topics=[TOPICS])])
    markets=[];seen=set();fee_events=set()
    for row in logs:
        key=(row["blockHash"],int(row["logIndex"],16))
        if row.get("removed") or row["address"].lower()!=factory or key in seen:raise ValueError("orphan, emitter or duplicate")
        seen.add(key)
        topics=row["topics"]
        if topics[0]==TOPICS[1]:
            fee=int(topics[1],16);spacing=decode(["int24"],bytes.fromhex(topics[2][2:]))[0]
            if fee in fee_events or tiers.get(fee,spacing)!=spacing: raise ValueError("duplicate or changed tier")
            fee_events.add(fee)
            tiers[fee]=spacing
        else:
            a,b=["0x"+t[-40:] for t in topics[1:3]];fee=int(topics[3],16);spacing,pool=decode(["int24","address"],bytes.fromhex(row["data"][2:]))
            salt=keccak(encode(["address","address","uint24"],[a,b,fee]))
            predicted="0x"+keccak(b"\xff"+bytes.fromhex(deployer[2:])+salt+bytes.fromhex(init[2:]))[-20:].hex()
            if a>=b or pool!=predicted or tiers.get(fee)!=spacing:raise ValueError(("CREATE2/tier",pool))
            markets.append(dict(pool=pool,tokens=[a,b],fee=fee,spacing=spacing,tx=row["transactionHash"],hash=row["blockHash"],index=int(row["logIndex"],16)))
    nonce=int(rpc("eth_getTransactionCount",[deployer,tag]),16)
    if len({r["pool"] for r in markets})!=len(markets) or nonce!=1+len(markets):
        raise ValueError(("creation count",nonce,len(markets)))
    for value,(fee,spacing) in zip(views([(factory,"feeAmountTickSpacing(uint24)",["uint24"],[fee]) for fee in tiers]),tiers.items(),strict=True):
        if decode(["int24"],value)[0]!=spacing:raise ValueError(("spacing",fee))
    fields=["factory","token0","token1","fee","tickSpacing","liquidity","slot0"]+(["lmPool"] if name=="vvs" else [])
    queries=[]
    for row in markets:
        pool=row["pool"];a,b=row["tokens"];fee=row["fee"]
        queries.extend((pool,f+"()",[],[]) for f in fields)
        queries.extend((factory,"getPool(address,address,uint24)",["address","address","uint24"],[x,y,fee]) for x,y in [(a,b),(b,a)])
        queries.extend((t,"balanceOf(address)",["address"],[pool]) for t in (a,b))
    vals=iter(views(queries));addr=lambda v:decode(["address"],v)[0]
    for row in markets:
        vals0={f:next(vals) for f in fields};pool=row["pool"]
        if [addr(vals0[f]) for f in fields[:3]]!=[factory,*row["tokens"]] or [int.from_bytes(vals0[f],"big") for f in fields[3:5]]!=[row["fee"],row["spacing"]]:
            raise ValueError(("pool identity",pool))
        if any(addr(v)!=pool for v in [next(vals),next(vals)]):raise ValueError(("registry",pool))
        row.update(liquidity=str(int.from_bytes(vals0["liquidity"],"big")),slot0="0x"+vals0["slot0"].hex(),balances=[str(int.from_bytes(next(vals),"big")) for _ in range(2)])
        if name=="vvs":row["lm_pool"]=addr(vals0["lmPool"])
    extra={}
    if name == "shibaswap-v2":
        from v3_factory_storage import verify_directory
        proof = rpc("eth_getProof", [factory, [], tag])
        owner, = decode(["address"],views([(factory,"owner()",[],[])])[0])
        extra["directory_proof"] = dict(factory=factory,state_root=head["stateRoot"],nonce=nonce,
            storage_root=proof["storageHash"],account_proof=proof["accountProof"],owner=owner,
            pool_pair_hash=init,fee_collector=collector,layout=dict(owner=3,poolPairHash=4,feeCollector=5,
            feeAmountTickSpacing=6,getPool=7),tiers=tiers,storage_slots=2*len(markets)+len(tiers)+1+
            int(int(owner,16)>0)+int(int(collector,16)>0))
        verify_directory(dict(extra["directory_proof"],pools=markets),factory)
    if name=="vvs":
        extra["rewards"]=reward_roles(name,markets,tag,rpc)
        exact="0x839e71613f9aa06e5701cf6de63e303616b0dde3"
        url="https://sourcify.dev/server/v2/contract/1/"+exact
        response=session.get(url,params={"fields":"compilation,runtimeBytecode.onchainBytecode"},timeout=20)
        response.raise_for_status();source=response.json()
        code=rpc("eth_getCode",[exact,tag])
        if (source["compilation"]["name"]!="CosmosERC20" or code.lower()!=source["runtimeBytecode"]["onchainBytecode"].lower()
            or "0x"+keccak(hexstr=code).hex()!="0x103268928496332938eeac5ecf9f2d1e1cbbc90af92ed12102a3b8a7fd1f6e40"):
            raise ValueError("different exact transfer implementation")
        extra["exact_token"]=dict(address=exact,runtime="0x"+keccak(hexstr=code).hex(),contract="CosmosERC20",source=url)
    return dict(**extra,chain="ethereum",chain_id=1,selector=tag,state_root=ROOT_HASH,height=snapshot["block"],factory=factory,
                deployer=deployer,roles=roles,nonce=nonce,init=init,tiers=tiers,markets=markets,events=logs)


def reward_roles(name,markets,tag,rpc):
    if name!="vvs":return []
    roles=[]
    for m in markets:
        address=m["lm_pool"]
        if int(address,16)==0:continue
        def address_getter(to,sig):
            raw=rpc("eth_call",[{"to":to,"data":calldata(sig,[],[])},tag])
            return decode(["address"],bytes.fromhex(raw[2:]))[0]
        def verified(to,expected_name):
            url="https://sourcify.dev/server/v2/contract/1/"+to
            reply=requests.get(url,params={"fields":"compilation,runtimeBytecode.onchainBytecode"},timeout=25)
            reply.raise_for_status();source=reply.json()
            code=rpc("eth_getCode",[to,tag])
            if code=="0x" or source["compilation"]["name"]!=expected_name or code.lower()!=source["runtimeBytecode"]["onchainBytecode"].lower():
                raise ValueError(("reward role source differs",to))
            return "0x"+keccak(hexstr=code).hex()
        pool=address_getter(address,"pool()")
        chef=address_getter(address,"masterChef()")
        if pool!=m["pool"] or int(chef,16)==0:raise ValueError("reward module bound to different pool")
        slot="0x"+(int.from_bytes(keccak(text="eip1967.proxy.implementation"),"big")-1).to_bytes(32,"big").hex()
        implementation=decode(["address"],bytes.fromhex(rpc("eth_getStorageAt",[chef,slot,tag])[2:]))[0]
        if int(implementation,16)==0:raise ValueError("unknown rewards implementation")
        roles.append(dict(pool=pool,hook=address,runtime=verified(address,"VVSV3LmPool"),chef=chef,
                          chef_runtime=verified(chef,"ERC1967Proxy"),implementation=implementation,
                          implementation_runtime=verified(implementation,"MasterChefV3")))
    return roles


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("venue",choices=FACTORIES);parser.add_argument("output",type=Path)
    parser.add_argument("--endpoint",default="https://mainnet.gateway.tenderly.co")
    args=parser.parse_args();result=capture(args.venue,args.endpoint)
    args.output.write_text(json.dumps(result,separators=(",",":"))+"\n",encoding="utf8")
