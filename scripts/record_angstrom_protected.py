"""Проверяет полную оплату по аттестации публичного узла через вновь созданный в симуляции модуль."""
from eth_abi import encode
from eth_utils import keccak
from record_angstrom_ethereum import ADAPTER, HOOK, MANAGER, rpc
from record_base_slipstream_executor import OWNER, EXEC, simulate_block
from record_bitswap_ethereum import tx
from record_diamondswap_base import direct_wallet

KEY = "(address,address,uint24,int24,address)"


def settlement(artifact, url, parent):
    if int(rpc(url, "eth_getTransactionCount", [OWNER, parent]), 16):
        raise ValueError("Test payer has historical transactions")
    args = encode(["address"]*3, [ADAPTER,HOOK,MANAGER])
    init = bytes.fromhex(artifact["creation"].removeprefix("0x")) + args
    creation = {"from":OWNER,"data":"0x"+init.hex(),"gas":hex(4_000_000)}
    module = "0x" + keccak(b"\xd6\x94"+bytes.fromhex(OWNER[2:])+b"\x80")[-20:].hex()
    if rpc(url, "eth_getCode", [module,parent]) != "0x":
        raise ValueError("Module CREATE address already in use")
    runtime = bytes.fromhex(rpc(url, "eth_call", [creation,parent])[2:])
    template = bytes.fromhex(artifact["runtime"].removeprefix("0x"))
    actual = bytearray(runtime)
    roles = set()
    for refs in artifact["immutableReferences"].values():
        values=set()
        for field in refs:
            start, end = field["start"], field["start"]+field["length"]
            values.add(runtime[start:end])
            actual[start:end] = template[start:end]
        if len(values)!=1: raise ValueError("Inconsistent immutable")
        roles.update(values)
    if actual != template or roles != {args[i:i+32] for i in range(0,len(args),32)}:
        raise ValueError("Protected module roles mismatch")
    return module, creation, {"target":module,"runtime":"0x"+keccak(runtime).hex(),"solc":artifact["solc"],
                           "constructor_roles":[ADAPTER,HOOK,MANAGER],"runtime_bytes":len(runtime)}


def payments(c, env, module, creation, market, key, block, data, stamp, weth, funding, ordinary):
    def action(x, y, quantity, minimum, end, _mode, height=block):
        return tx(module,"execute("+KEY+",bool,uint128,uint128,uint64,bytes,uint256)",
                  [KEY,"bool","uint128","uint128","uint64","bytes","uint256"],
                  [key,x==key[0],quantity,minimum,height,data,end])
    def wrap(n):
        return [creation,tx(weth,"deposit()",value=n)]
    stable = next(t for t in key[:2] if t!=weth)
    checked=[]
    for a in ordinary:
        size=int(a["size"])
        other=a["input"]!=weth
        fund_sale=(lambda n:[creation,*funding(n)]) if other else None
        try:
            trial,=direct_wallet(c,market,[size],stamp,0,module,wrapped=weth,
                funding=wrap,fund_sale=fund_sale,audit=True,keep_partial=True,modes=["router"],build_action=action)
        except ValueError as error:
            if "reverted" not in a: raise
            if not str(error).startswith("could not settle router:"): raise
            checked.append(dict(pool=a["pool"],input=a["input"],size=a["size"],reverted=str(error)[:260]))
            continue
        for leg in ("buy","sell"):
            if (leg in trial)!=(leg in a): raise ValueError("Protected/original coverage differs")
            if leg in a:
                for field in ("received","spent"):
                    if trial[leg]["exact"][field]!=a[leg]["exact"][field]:
                        raise ValueError("Protected/original cashflow differs")
        checked.append(dict(pool=a["pool"],input=a["input"],size=a["size"],**trial))
    budget=10**12
    allowance=tx(weth,"allowance(address,address)",["address","address"],[module,ADAPTER])
    residue=tx(weth,"balanceOf(address)",["address"],[module])
    prefix=wrap(budget)+[tx(weth,"approve(address,uint256)",["address","uint256"],[module,budget])]
    calls=prefix+[action(weth,stable,budget,1,stamp+1,None,block-1),action(weth,stable,budget,1,stamp+1,None),allowance,residue]
    answers=simulate_block(c.endpoint,c.selector,env,calls)
    if any(r["status"]!="0x1" for r in answers[:3]+answers[4:]) or answers[3]["status"]!="0x0" or any(int(r["returnData"],16) for r in answers[-2:]):
        raise ValueError("Height, allowance or residue boundary")
    return checked, dict(pool=market["pool"],creation_gas=answers[0]["gasUsed"],wrong_height=answers[3],allowance=answers[-2]["returnData"],residue=answers[-1]["returnData"])
