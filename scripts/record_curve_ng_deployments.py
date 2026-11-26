"""Записывает Curve NG из исходных манифестов и eth_call на закреплённых блоках.

Каталог Curve API даёт кандидатов, фабрика, версия, байткод MATH и get_dy подтверждаются на узле.
В профиль попадают только семейства с подтверждённой математикой и двусторонней котировкой.
"""
import hashlib
import re
import sys

from eth_utils import keccak
import requests
import yaml

from record_balancer_deployments import ROOT, read, write
from record_balancer_markets import Capture

SDK = "ab3f1c244e5686ff7cc6fa9f0d4bfe53b8e6e374"
TWO = "89c8dfb6c9b3204647954018d2a0238b9c69349d"
TRI = "ecaa8161c240f21dd7c3712eefc5637e1dac742b"
SOURCES = {
    "stable-ng": "https://github.com/curvefi/curve-js/blob/" + SDK + "/src/constants/network_constants.ts",
    "twocrypto": "https://github.com/curvefi/twocrypto-ng/blob/" + TWO + "/deployments.yaml",
    "tricrypto": "https://github.com/curvefi/tricrypto-ng/blob/" + TRI + "/deployments.yaml",
}


def source(kind):
    url = SOURCES[kind].replace("github.com","raw.githubusercontent.com").replace("/blob/","/")
    response = requests.get(url, timeout=20)
    response.raise_for_status()
    return response.text


def code(e, address):
    payload = dict(jsonrpc="2.0", id=1, method="eth_getCode", params=[address, e["selector"]])
    response = requests.post(e["endpoint"],json=payload,timeout=20)
    response.raise_for_status()
    raw = bytes.fromhex(response.json()["result"][2:])
    if not raw:
        raise ValueError(f"{address}: пустой код")
    obj = {"bytes":len(raw), "keccak256":"0x"+keccak(raw).hex()}
    e["contracts"][address.lower()] = obj
    return obj["keccak256"]


def verified_mainnet_math(e, address):
    """Сверяет текст формул Vyper, включая отступы, и код RPC с проверенной записью Sourcify."""
    url = f"https://sourcify.dev/server/v2/contract/1/{address}?fields=sources,compilation,runtimeBytecode.onchainBytecode"
    response = requests.get(url, timeout=20)
    response.raise_for_status()
    verified = response.json()
    upstream = f"https://raw.githubusercontent.com/curvefi/tricrypto-ng/{TRI}/contracts/main/CurveCryptoMathOptimized3.vy"
    response = requests.get(upstream, timeout=20)
    response.raise_for_status()
    actual = verified["sources"]["CurveTricryptoMathOptimized.vy"]["content"].splitlines()
    expected = response.text.splitlines()
    if actual[0] != "# @version 0.3.9" or expected[:3] != [
        "# pragma version 0.3.10", "# pragma optimize gas", "# pragma evm-version paris"]:
        raise ValueError("неизвестные директивы компилятора")
    digest = "0x" + keccak(bytes.fromhex(verified["runtimeBytecode"]["onchainBytecode"][2:])).hex()
    if (str(verified["chainId"]) != "1" or verified["address"].lower() != address.lower()
            or verified["runtimeMatch"] not in ("match","exact_match")
            or digest != e["contracts"][address.lower()]["keccak256"] or actual[1:] != expected[3:]):
        raise ValueError("исходник, снимок runtime или сеть расходятся")
    return dict(verified_source=url, canonical_source=upstream, runtime_match=verified["runtimeMatch"],
                deployed_compiler=verified["compilation"]["compilerVersion"], recorded_runtime_keccak=digest,
                source_sha256_after_compiler_directives=hashlib.sha256("\n".join(actual[1:]).encode()).hexdigest())


def bot_state(c, pool, n):
    """Снимает те же данные, что читает load_crypto с дополнительными параметрами tweak_price."""
    sys.path.insert(0, str(ROOT / "tools/arbitrum_record"))
    from curve_crypto_pools import bot_calls
    calls = []
    for signature, arg in bot_calls(n, False):
        calls.append((pool, signature, ["uint256"] if arg else [], [int(arg, 16)] if arg else []))
    out = {}
    for start in range(0, len(calls), 3):
        batch = calls[start:start + 3]
        replies = c.batch(batch)
        for recorded, reply in zip(c.calls[-len(batch):], replies, strict=True):
            if reply is None:
                raise ValueError("загрузка состояния: " + recorded["signature"])
            out[recorded["data"]] = {"result": "0x" + reply.hex()}
    return out


def pool_quotes(c, network, kind, factory, allowed_math):
    url = f"https://api.curve.finance/v1/getPools/{network}/factory-{kind}"
    r = requests.get(url,timeout=20)
    r.raise_for_status()
    pp = sorted(r.json()["data"]["poolData"],key=lambda p:float(p.get("usdTotal") or 0),reverse=True)
    stable = kind == "stable-ng"
    coin_type = {"stable-ng":"address[]","twocrypto":"address[2]","tricrypto":"address[3]"}[kind]
    rows = []
    supported = {"v7.0.0"} if stable else ({"v2.1.0","v2.0.0"} if kind=="twocrypto" else {"v2.0.0"})
    for p in pp[:20]:
        pool = p["address"].lower()
        row = dict(pool=pool,quotes=[])
        rows.append(row)
        try:
            coins, = c.one(factory, "get_coins(address)",["address"],[pool],[coin_type])
            if len(coins)<2 or any(int(t,16)==0 for t in coins):
                raise ValueError("монеты не подтверждены")
            version, = c.one(pool,"version()",returns=["string"])
            row.update(coins=coins,version=version)
            if version not in supported:
                raise ValueError("другая версия")
            if stable:
                meta, = c.one(factory, "is_meta(address)", ["address"], [pool], ["bool"])
                if meta:
                    raise ValueError("метапул")
            else:
                math, = c.one(pool,"MATH()",returns=["address"])
                row["math"] = math
                if math not in allowed_math:
                    raise ValueError("неподтверждённый вариант MATH")
            indices = "int128" if stable else "uint256"
            for i,j in ((0,1),(1,0)):
                bal, = c.one(pool,"balances(uint256)",["uint256"],[i],["uint256"])
                for fraction in (100_000, 1_000):
                    amount = bal // fraction
                    out, = c.one(pool,f"get_dy({indices},{indices},uint256)",[indices,indices,"uint256"],[i,j,amount],["uint256"])
                    if out == 0:
                        raise ValueError("нулевая котировка")
                    row["quotes"].append(dict(i=i,j=j,amount_in=str(amount),amount_out=str(out)))
            if not stable:
                row["calls"] = bot_state(c, pool, len(coins))
                row["get_dy"] = [dict(i=q["i"],j=q["j"],dx=q["amount_in"],
                                 get_dy={"result":"0x"+int(q["amount_out"]).to_bytes(32,"big").hex()}) for q in row["quotes"]]
            row["status"] = "quoted"
        except ValueError as err:
            row.update(status="excluded",reason=str(err))
        if sum(m["status"]=="quoted" for m in rows) == 2:
            break
    return dict(candidate_source=url, pools=rows)


def main():
    folder = ROOT/"tests/data/first_run_dex_2026-11-26"
    target = ROOT/"config/first_run_dex.json"
    sdk = source("stable-ng")
    manifests = {k:yaml.safe_load(source(k)) for k in ("twocrypto","tricrypto")}
    new = []
    collected = []
    for net in ("base","ethereum"):
        e = read(folder/f"{net}.json")
        parent = "curve-base" if net=="base" else "curve"
        aliases = sdk.split(f"const ALIASES_{net.upper()} = lowerCaseValues(")[1].split("});")[0]
        by_kind = {}
        for kind in SOURCES:
            c = Capture(e)
            key = "stable_ng_factory" if kind=="stable-ng" else kind+"_factory"
            f = re.search('"'+key+'"'+r":\s*'([^']+)'",aliases)[1]
            code(e,f)
            settings = {"dex.kind":"curve_"+("stableswap_ng" if kind=="stable-ng" else kind+"_ng"),"dex.factory":f}
            ref = {}
            if kind=="stable-ng":
                settings["dex.curve_ng_versions"] = ["v7.0.0"]
            else:
                m = manifests[kind][net+":mainnet"]
                if m["factory"].lower() != f.lower():
                    raise ValueError("манифест и SDK указывают разные фабрики")
                original = read(ROOT/f"tests/data/arbitrum_curve_crypto_2026-11-20/{kind}.json")
                allowed = []
                for i, role in enumerate(("math","math_v2_0_0") if kind=="twocrypto" else ("math",)):
                    canonical = bytes.fromhex(original["code_of"][original["maths"][i]]["code"][2:])
                    expected = "0x"+keccak(canonical).hex()
                    observed = code(e,m[role])
                    matched = expected == observed
                    ref[role] = dict(address=m[role], expected=expected, observed=observed, bytecode_matches=matched)
                    if not matched and net=="ethereum" and kind=="tricrypto":
                        ref[role]["source"] = verified_mainnet_math(e,m[role])
                        matched = True
                    if matched:
                        settings["dex.curve_crypto_"+("math" if i==0 else "math_v2_0_0")] = [m[role]]
                        allowed.append(m[role].lower())
                if not allowed:
                    by_kind[kind] = dict(factory=f, math=ref, status="different_math_bytecode")
                    continue
                settings["dex.curve_crypto_versions"] = ["v2.1.0","v2.0.0"] if kind=="twocrypto" else ["v2.0.0"]
            markets = pool_quotes(c,net,kind,f,allowed if not kind=="stable-ng" else [])
            quoted = any(p["status"]=="quoted" for p in markets["pools"])
            by_kind[kind] = dict(factory=f, math=ref, **markets, calls=c.calls, status="quoted" if quoted else "unconfirmed")
            if quoted:
                new.append(dict(chain_id=e["chain_id"], network=net, id=parent if kind=="stable-ng" else parent+"-"+kind+"-ng",
                                source=SOURCES[kind], settings=settings))
        main = next(r for r in new if r["network"]==net and r["id"]==parent)
        main["settings"]["dex.sub_dex"] = [r["id"] for r in new if r["network"]==net and r["id"]!=parent]
        collected.append((folder/f"{net}.json",e))
        head = requests.post(e["endpoint"],json=dict(jsonrpc="2.0",id=1,method="eth_getBlockByHash",
                             params=[e["selector"]["blockHash"],False]),timeout=20).json()["result"]
        collected.append((folder/f"curve_{net}.json",dict(chain_id=e["chain_id"],endpoint=e["endpoint"],selector=e["selector"],
                                                          timestamp=int(head["timestamp"],16),families=by_kind)))
        print(net,[(k,d["status"]) for k,d in by_kind.items()],flush=True)
    for path,value in collected:
        write(path,value)
    ids = {(r["network"],r["id"]) for r in new}
    write(target,[r for r in read(target) if (r["network"],r["id"]) not in ids]+new)


if __name__=="__main__":
    main()
