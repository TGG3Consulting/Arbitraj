"""Файл: tools/arbitrum_record/balancer_v2_more_calls.py
Что делает: решение 473 — вызовы бота к пулу Balancer V2 вида dex.balancer_pool (те же и в том же порядке, что
  src/venues/bsc_dex/pools/load_balancer_more_fixed.rs read и load_balancer_more.rs calls) и разбор нужных скрипту
  ответов ABI (uint, address[], динамические массивы getPoolTokens). Запускать не нужно — его импортирует
  balancer_v2_more.py.
Кто вызывает: tools/arbitrum_record/balancer_v2_more.py.
От чего зависит: curve_crypto_rpc.py (keccak256, selector, word, addr_word).
Ошибки: разбор — ValueError на ответе не по ABI.
"""

from curve_crypto_rpc import addr_word, selector, word

ZERO = "0x" + "00" * 20
STABLE = ("composable_stable", "stable_phantom", "meta_stable")
WEIGHTED = ("lbp", "investment", "managed")
LINEAR = ("linear_v1", "linear")


def c(to, sig, *args):
    """Вызов: адрес, сигнатура, слова аргументов (hex без 0x). Ошибок нет."""
    return {"name": sig, "to": to, "data": selector(sig) + "".join(args)}


def words(h):
    """Ответ eth_call → слова по 32 байта (hex без 0x). Ошибок нет."""
    h = h[2:] if h.startswith("0x") else h
    return [h[i:i + 64] for i in range(0, len(h), 64)]


def dyn(h, k):
    """k-й динамический массив ответа (слова). Ошибки: ValueError — не по ABI."""
    ws = words(h)
    off = int(ws[k], 16) // 32
    n = int(ws[off], 16)
    if off + 1 + n > len(ws):
        raise ValueError("массив за пределами ответа")
    return ws[off + 1:off + 1 + n]


def addr(w):
    """Слово → адрес 0x… (нижний регистр). Ошибок нет."""
    return "0x" + w[-40:]


def fixed_calls(ty, pool, factory, vault):
    """Первый проход неизменяемого (до токенов): getPoolId, isPoolFromFactory фабрики ключа. Ошибок нет."""
    return [c(pool, "getPoolId()"), c(factory, "isPoolFromFactory(address)", addr_word(pool))]


def type_calls(ty, pool):
    """Неизменяемое вида (load_balancer_more_fixed.rs read, тот же порядок). Ошибок нет."""
    if ty == "composable_stable":
        return [c(pool, "getBptIndex()"), c(pool, "getRateProviders()")]
    if ty == "stable_phantom":
        return [c(pool, "getBptIndex()")]
    if ty == "meta_stable":
        return [c(pool, "getRateProviders()")]
    if ty in LINEAR:
        return [c(pool, "getMainIndex()"), c(pool, "getWrappedIndex()"), c(pool, "getBptIndex()")]
    return []


def batch_calls(ty, pool, vault, pid, tokens, providers, bpt, multicall):
    """Пачка каждого блока (load_balancer_more.rs calls, тот же порядок). Принимает: вид, пул, хранилище, id пула (hex
    0x…), токены, поставщики курсов (или []), индекс BPT (или None), Multicall3. Ошибок нет."""
    if ty in WEIGHTED:
        curve = c(pool, "getNormalizedWeights()")
    elif ty in LINEAR:
        curve = c(pool, "getTargets()")
    else:
        curve = c(pool, "getAmplificationParameter()")
    out = [c(vault, "getPoolTokens(bytes32)", pid[2:]), c(pool, "getSwapFeePercentage()"),
           c(pool, "getPausedState()"), curve, c(vault, "getPausedState()")]
    rated = [(t, p) for t, p in zip(tokens, providers) if p != ZERO]
    if ty == "composable_stable":
        out.append(c(pool, "getScalingFactors()"))
    if ty in ("composable_stable", "meta_stable"):
        cache = "getTokenRateCache(address)" if ty == "composable_stable" else "getPriceRateCache(address)"
        for t, p in rated:
            out.append(c(pool, cache, addr_word(t)))
            out.append(c(p, "getRate()"))
        if rated:
            out.append(c(multicall, "getCurrentBlockTimestamp()"))
    elif ty == "stable_phantom":
        out += [c(pool, "getScalingFactor(address)", addr_word(t)) for t in tokens]
    elif ty == "lbp":
        out.append(c(pool, "getSwapEnabled()"))
    elif ty == "investment":
        out += [c(pool, "getSwapEnabled()"), c(pool, "getCollectedManagementFees()")]
    elif ty == "managed":
        out += [c(pool, "getSwapEnabled()"), c(pool, "getActualSupply()")]
        out += [c(pool, "getCircuitBreakerState(address)", addr_word(t)) for k, t in enumerate(tokens) if k != bpt]
    elif ty in LINEAR:
        out.append(c(pool, "getWrappedTokenRate()"))
        if ty == "linear":
            out.append(c(pool, "totalSupply()"))
    return out


def query_batch_swap(vault, pid, i, j, amount, tokens, me):
    """Vault.queryBatchSwap(GIVEN_IN, один шаг i → j, assets = токены пула, funds = me) — IVault.sol 20210418-vault
    L587-L593, L623-L628, L645-L650 (тот же вызов, что quote/quoter_balancer.rs). Ошибок нет."""
    sig = "queryBatchSwap(uint8,(bytes32,uint256,uint256,uint256,bytes)[],address[],(address,bool,address,bool))"
    head = 32 * 7
    step = word(32) + pid[2:] + word(i) + word(j) + word(amount) + word(160) + word(0)
    swaps = word(1) + step
    assets_off = head + len(swaps) // 2
    assets = word(len(tokens)) + "".join(addr_word(t) for t in tokens)
    data = (selector(sig) + word(0) + word(head) + word(assets_off) + addr_word(me) + word(0) + addr_word(me)
            + word(0) + swaps + assets)
    return {"name": "queryBatchSwap", "to": vault, "data": data, "i": i, "j": j, "amount": str(amount)}
