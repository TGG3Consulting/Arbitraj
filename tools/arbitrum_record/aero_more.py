"""Файл: tools/arbitrum_record/aero_more.py
Что делает: решение 470 (T470-01, T470-05, T470-06) — дополнения записи aero.py на ТОМ ЖЕ блоке (только eth_call):
  - пулы Slipstream V3: данные первой комиссии нового блока крючка DynamicSwapFeeHook — secondsAgo,
    defaultScalingFactor, defaultFeeCap крючка (hook_params), dynamicFeeConfig(пул), blockFee(пул, блок),
    observations((index + 1) % cardinality) и observations(0), observe([secondsAgo − δ, 0]) для δ из AHEAD;
    fee() пула с blockOverrides {time: время блока + δ} и {time, number: блок + 1} (комиссия, которую возьмёт первый
    обмен следующего блока) — для сверки порта pools/aero_first_fee.rs; балансы пула и котировки на доли баланса в обе
    стороны — Metaquoter quoteExactInputSingleV3(pool, tokenIn, amount, 0) (T470-05) и виртуальный квотер бота
    (quoteExactIn(pool, zeroForOne, amount), код contracts/out/VirtualQuoter.bin-runtime подменой по адресу
    keccak256("arb.virtual_quoter")[12:] — как quote/virtual_quoter.rs), на блоке и с blockOverrides.time + δ;
  - пулы Basic: getAmountOutWithTotalFee пула (шаг AERO_BASIC, T470-06) и Metaquoter quoteExactInputSingleV2 на те
    же суммы, что getAmountOut в aero.py.
Селекторы считает сам узел (web3_sha3) из текстов сигнатур — чисел в скрипте нет.
Источники: Dromos-Labs/metadex-public 79c8d5b6 (DynamicSwapFeeHook.sol, IDynamicSwapFeeHook.sol, Pool.sol L548-L555),
Dromos-Labs/metadex-slipstream-public cf1ae46f (ICLPoolState.sol, CLPool.sol L190-L194, IMetaquoter.sol,
deployment-addresses/cl-arbitrum.json «metaquoter» 0x1dDfeC5058180C207F55BB77D95AcDd0d8227868).
Кто вызывает: aero.py. От чего зависит: стандартная библиотека; объект Rpc из aero.py (raw).
"""

import os

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
METAQUOTER = "0x1dDfeC5058180C207F55BB77D95AcDd0d8227868"
AHEAD = (0, 1, 3)  # время блока + секунды: первый обмен следующего блока (как tools/bsc_record/thena_fusion.py)
BPS = (1, 10, 100)  # сумма = баланс входа пула × bps / 10000
VQ_RUNTIME = os.path.join(ROOT, "contracts", "out", "VirtualQuoter.bin-runtime")
_SEL = {}


def sel(rpc, sig):
    """Селектор (8 hex без 0x) — keccak256 сигнатуры у самого узла (web3_sha3)."""
    if sig not in _SEL:
        _SEL[sig] = rpc.raw("web3_sha3", ["0x" + sig.encode().hex()])["result"][2:10]
    return _SEL[sig]


def word(v):
    return format(v % (1 << 256), "064x")


def addr(a):
    return word(int(a, 16))


def words(res):
    raw = res.get("result", "0x")[2:]
    return [int(raw[i:i + 64], 16) for i in range(0, len(raw) - 63, 64)]


def signed(v, bits):
    v &= (1 << bits) - 1
    return v - (1 << bits) if v >> (bits - 1) else v


def call(rpc, to, data, tag, state=None, block=None):
    """eth_call с необязательными stateOverrides и blockOverrides; откат — {"error": …} в записи."""
    params = [{"to": to, "data": "0x" + data}, tag]
    if state is not None or block is not None:
        params.append(state or {})
    if block is not None:
        params.append(block)
    body = rpc.raw("eth_call", params)
    return {"error": body["error"]} if "error" in body else {"result": body["result"]}


def hook_params(rpc, hook, tag):
    return {k: call(rpc, hook, sel(rpc, k + "()"), tag) for k in ("secondsAgo", "defaultScalingFactor",
                                                                     "defaultFeeCap", "mevTaxModule")}


def vq(rpc):
    """Адрес и код виртуального квотера бота (как quote/virtual_quoter.rs)."""
    a = "0x" + rpc.raw("web3_sha3", ["0x" + b"arb.virtual_quoter".hex()])["result"][-40:]
    with open(VQ_RUNTIME, encoding="utf-8") as f:
        return a, "0x" + f.read().strip().replace("0x", "")


def cl_first_fee(rpc, hook, x, tag, head, params, vq_pair):
    """Дописывает в запись пула CL x данные первой комиссии блока и котировки (см. шапку)."""
    p, num, ts = x["pool"], int(head["number"], 16), int(head["timestamp"], 16)
    s0 = words(x["slot0"])
    secs = words(params["secondsAgo"])
    x["dynamicFeeConfig"] = call(rpc, hook, sel(rpc, "dynamicFeeConfig(address)") + addr(p), tag)
    x["blockFee"] = call(rpc, hook, sel(rpc, "blockFee(address,uint256)") + addr(p) + word(num), tag)
    if len(s0) >= 4 and s0[3] > 0:
        obs = sel(rpc, "observations(uint256)")
        x["observations_next"] = call(rpc, p, obs + word((s0[2] + 1) % s0[3]), tag)
        x["observations_0"] = call(rpc, p, obs + word(0), tag)
    if secs:
        ob = sel(rpc, "observe(uint32[])")
        x["observe"] = {str(d): call(rpc, p, ob + word(32) + word(2) + word(secs[0] - d) + word(0), tag)
                        for d in AHEAD if d <= secs[0]}
    fee = sel(rpc, "fee()")
    x["fee_at"] = {str(d): {"time": call(rpc, p, fee, tag, block={"time": hex(ts + d)}),
                            "time_number": call(rpc, p, fee, tag, block={"time": hex(ts + d), "number": hex(num + 1)})}
                   for d in AHEAD}
    toks = [words(x[k])[0] if words(x[k]) else 0 for k in ("token0", "token1")]
    bal = [words(call(rpc, "0x" + format(t, "040x"), sel(rpc, "balanceOf(address)") + addr(p), tag)) for t in toks]
    x["balances"] = [b[0] if b else None for b in bal]
    mq = sel(rpc, "quoteExactInputSingleV3(address,address,uint256,uint160)")
    vq_sel = sel(rpc, "quoteExactIn(address,bool,uint256)")
    vq_addr, vq_code = vq_pair
    x["quotes_more"] = []
    for zfo in (True, False):
        tin, b = (toks[0], x["balances"][0]) if zfo else (toks[1], x["balances"][1])
        for bps in BPS:
            amt = (b or 0) * bps // 10000
            if amt == 0:
                continue
            q = {"zero_for_one": zfo, "bps": bps, "amount_in": str(amt)}
            for d in (None,) + AHEAD[1:]:
                over = None if d is None else {"time": hex(ts + d)}
                key = "at_block" if d is None else f"at_plus_{d}"
                q[key] = {
                    "metaquoter": call(rpc, METAQUOTER, mq + addr(p) + word(tin) + word(amt) + word(0), tag, block=over),
                    "virtual": call(rpc, vq_addr, vq_sel + addr(p) + word(int(zfo)) + word(amt), tag,
                                    state={vq_addr: {"code": vq_code}}, block=over),
                }
            x["quotes_more"].append(q)


def basic_quote_more(rpc, pool, tok, amt, tag):
    """getAmountOutWithTotalFee пула и Metaquoter quoteExactInputSingleV2 на ту же сумму (см. шапку)."""
    t = sel(rpc, "getAmountOutWithTotalFee(uint256,address)")
    m = sel(rpc, "quoteExactInputSingleV2(address,address,uint256)")
    return {"getAmountOutWithTotalFee": call(rpc, pool, t + word(amt) + addr(tok), tag),
            "metaquoter": call(rpc, METAQUOTER, m + addr(pool) + addr(tok) + word(amt), tag)}
