"""Приёмка ArbExecutor v5 в BSC: пулы PancakeSwap Infinity CL без крючка (вид 20).

Используются только три публичных PoolId из tests/data/executor_v3_live_2026-11-17.
Ключ каждого пула и адрес Vault считываются заново на блоке записи; выбор фиксирован до
симуляции. ABI ключа, квотера и Swap те же, что в записи v3, scripts/main2.py.
Записывающая программа передаёт эти функции общему сценарию v4live_cases.case_ab.
"""

from curve_crypto_rpc import keccak256
from v4live_rpc import ZERO, adr, sel, sword, topic, w, word


CL_MANAGER = "0xa0ffb9c1ce1fe56963b0321b32e7a0302114058b"
CL_VAULT = "0x238a358808379702088667322f80ac48bad5e6c4"
CL_QUOTER = "0xd0737c9762912dd34c3271197e362aa736df0926"
POOL_IDS = (
    "0xa005f46e21d51102360f3e34e9b19c363e6b959ddc9849ac23ec9c13ec36171f",
    "0xdfe5344e064b6c7ea77724a1f5f0fe87fc8cfa0d55506a38560898e68e71eef8",
    "0xed4de4a3e800f8ae9fd35210fe75d984330dd598eacca45fef81a96e24307803",
)
QUOTE_SIGNATURE = "quoteExactInputSingle(((address,address,address,address,uint24,bytes32),bool,uint128,bytes))"
SWAP = topic("Swap(bytes32,address,int128,int128,uint160,uint128,int24,uint24,uint16)")


def pools(node, tag):
    """Проверяет vault() и ключи менеджера на H. Некорректный ответ прерывает запись."""
    value, err = node.call(CL_MANAGER, "0x" + sel("vault()"), tag)
    if not value or len(value) != 66 or adr(value, 0) != CL_VAULT:
        raise ValueError(f"Infinity CL: vault() на {tag}: {err or value}")
    result = []
    for pool_id in POOL_IDS:
        call = "0x" + sel("poolIdToPoolKey(bytes32)") + pool_id[2:]
        key, err = node.call(CL_MANAGER, call, tag)
        if not key or len(key) != 2 + 64 * 6:
            raise ValueError(f"Infinity CL {pool_id}: poolIdToPoolKey на {tag}: {err or key}")
        if "0x" + keccak256(bytes.fromhex(key[2:])).hex() != pool_id:
            raise ValueError(f"Infinity CL {pool_id}: keccak256(abi.encode(PoolKey)) не совпал")
        tokens = [adr(key, 0), adr(key, 1)]
        if adr(key, 2) != ZERO or adr(key, 3) != CL_MANAGER or ZERO in tokens:
            raise ValueError(f"Infinity CL {pool_id}: ожидался пул двух ERC-20 без крючка своего менеджера")
        result.append(dict(fam="infinity_cl", kind=20, pool_id=pool_id, tokens=tokens,
                           hooks=ZERO, manager=CL_VAULT, pool_manager=CL_MANAGER,
                           step_pool=CL_VAULT, data=key))
    return result


def quote_call(pool, args, wallet, token_in, token_out, amount):
    """Возвращает eth_call квотера и декодер количества выхода для выбранной пары."""
    if [token_in, token_out] not in (pool["tokens"], pool["tokens"][::-1]):
        raise ValueError(f"Infinity CL {pool['pool_id']}: токены запроса не образуют пару")
    zero_for_one = token_in == pool["tokens"][0]
    # Смещение пустого hookData внутри tuple: 6 слов PoolKey и три остальных поля.
    data = ("0x" + sel(QUOTE_SIGNATURE) + w(32) + pool["data"][2:] + w(zero_for_one)
            + w(amount) + w(32 * 9) + w(0))
    return {"to": CL_QUOTER, "from": wallet, "input": data}, lambda rd: word(rd, 0)


def credit(pool, logs, token_out):
    """Кредит выхода из единственного события Swap указанного менеджера для этого PoolId."""
    matching = [log for log in logs if log["address"].lower() == CL_MANAGER
                and log.get("topics", [])[:2] == [SWAP, pool["pool_id"]]]
    if len(matching) != 1 or token_out not in pool["tokens"]:
        return None
    data = matching[0]["data"]
    if len(data) < 2 + 2 * 64:
        return None
    amounts = [sword(data, 0), sword(data, 1)]
    side = pool["tokens"].index(token_out)
    if amounts[side] <= 0 or amounts[1 - side] >= 0:
        return None
    return dict(amount0=amounts[0], amount1=amounts[1], out_side=amounts[side])
