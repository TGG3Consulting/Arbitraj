"""Файл: tools/arbitrum_record/wombat_calls.py
Что делает: решение 484 — общее для записи Wombat Exchange (wombat.py): официальные адреса пулов Arbitrum, вызовы бота
  по (адрес, calldata) — те же, что строят src/venues/bsc_dex/pools/load_wombat_calls.rs (head_calls, asset_calls,
  detail_calls; getRelativePrice — у всех активов: ответ или откат пишется как есть) — и слоты хранилища, которые
  читает load_wombat.rs (реализация прокси EIP-1967, PausableAssets._pausedAssets[монета] — слот 201), симуляция
  swap пула от пустого адреса с подменой баланса и разрешения (слоты — curve_sim.Sim).
Кто вызывает: tools/arbitrum_record/wombat.py.
От чего зависит: curve_crypto_rpc.py (selector, word, addr_word), curve_sim.py (Sim, SENDER, _allow_slot),
  v4live_rpc.py (sel, w, ad, MAXU, mapslot, decode_revert), v4live_slots.py (balance_of).
Адреса пулов — docs.wombat.exchange/developers/contracts/arbitrum (страница «Arbitrum», раздел Pool; прочитана
  20.11.2026). Реализации (Sourcify proxyResolution) — docs/sources/wombat_arbitrum/README.md.
"""

from curve_crypto_rpc import addr_word, selector
from curve_sim import SENDER, Sim, _allow_slot
from v4live_rpc import MAXU, ad, mapslot, sel, w
import v4live_slots as slots

POOLS = {
    "CrossChain": "0xe78876C360716f2225F55A6726B32324FE1B1145",
    "Main": "0xc6bc781E20f9323012F6e422bdf552Ff06bA6CD1",
    "Overnight": "0xCF20fDA54e37f3fB456930f02fb07FccF49e4849",
    "MIM": "0x29eeB257a2A6eCDE2984aCeDF80A1B687f18eC91",
    "BOB": "0x917caF2b4D6040a9D67A5F8CEfC4F89d1b214c1A",
    "mWOM": "0x90eCddEC4E4116E30769A4e1EA52c319aca338B6",
    "wmxWOM": "0xEE9b42b40852a53c7361F527e638B485D49750cD",
    "qWOM": "0x12Fa5AB079CFf564d599466d39715D35d90Af978",
    "frxETH": "0x20D7ee728900848752FA280fAD51aF40c47302f1",
    "wstETH": "0xe14302040c0A1eb6fB5A4A79EfA46D60029358d9",
    "ankrETH": "0xB9bdfE449Da096256Fe7954Ef61A18eE195Db77B",
    "FRAX-MAI-USD+": "0x4a8686df475D4c44324210FFA3Fc1DEA705296e0",
    "jUSDC": "0xc7a6bA5F28993BaDb566007bD2E0CB253c431974",
    "fUSDC": "0x956454C7BE9318863297309183C79b793D370401",
    "mPENDLE": "0xe7159f15e7b1d6045506B228A1ed2136dcc56F48",
    "USDV": "0xa6eF6C45EbFDBc13f6D032fbDFeC9b389C1603E5",
    "FRAX-USDV": "0x3cc8c886575968642Cab9F430261c81C5b044d4b",
}
# EIP-1967: keccak256("eip1967.proxy.implementation") − 1.
IMPL_SLOT = "0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc"
# PausableAssets._pausedAssets — слот 201 (Sourcify storageLayout всех развёрнутых реализаций; abi_wombat.rs).
PAUSED_SLOT = 201
HEAD = ["getTokens()", "ampFactor()", "haircutRate()", "startCovRatio()", "endCovRatio()", "paused()"]
DETAIL = ["underlyingToken()", "underlyingTokenDecimals()", "pool()", "cash()", "liability()", "getRelativePrice()"]
SWAP = "swap(address,address,uint256,uint256,address,uint256)"
QUOTE = "quotePotentialSwap(address,address,int256)"


def key(to, data):
    """Ключ ответа: «адрес|calldata» нижним регистром — тест ищет ответ на тот же вызов бота."""
    return to.lower() + "|" + data.lower()


def head_calls(pool):
    return [(pool, selector(s)) for s in HEAD]


def asset_call(pool, token):
    return (pool, selector("addressOfAsset(address)") + addr_word(token))


def detail_calls(asset):
    return [(asset, selector(s)) for s in DETAIL]


def paused_slot(token):
    """Слот _pausedAssets[монета] = keccak256(abi.encode(монета, 201))."""
    return mapslot(ad(token), PAUSED_SLOT)


def quote_data(t_in, t_out, dx):
    return "0x" + sel(QUOTE) + ad(t_in) + ad(t_out) + w(dx)


def tokens_of(hexres):
    """Разбор ответа getTokens() (address[]): смещение, длина, элементы."""
    h = hexres[2:]
    off = int(h[0:64], 16) * 2
    n = int(h[off:off + 64], 16)
    return ["0x" + h[off + 64 + 64 * k + 24:off + 128 + 64 * k] for k in range(n)]


class WSim(Sim):
    """Симуляция swap пула Wombat от пустого W: баланс монеты входа = текущий + dx, разрешение W → пул = 2^256 − 1;
    swap(from, to, dx, 1, W, 2^256 − 1) — минимум 1, как у шага WOMBAT исполнителя. Слоты — Sim._slots (поиск один раз на токен)."""

    def add_swap(self, pool, t_in, t_out, dx):
        rec = {"pool": pool, "from": t_in, "to": t_out, "dx": str(dx), "sig": SWAP}
        b, a, why = self._slots(t_in, pool)
        if b is None or a is None:
            rec.update(ok=None, error="слоты не найдены: " + str(why))
            self.recs.append((rec, None))
            return
        if t_in.lower() not in self.bal:
            self.bal[t_in.lower()] = slots.balance_of(self.node, self.tag, t_in, SENDER) or 0
        bal = self.bal[t_in.lower()]
        ov = {t_in: {"stateDiff": {b[2]: "0x" + w(bal + dx), _allow_slot(a[0], a[1], SENDER, pool): "0x" + w(MAXU)}}}
        data = "0x" + sel(SWAP) + ad(t_in) + ad(t_out) + w(dx) + w(1) + ad(SENDER) + w(MAXU)
        q = "0x" + sel("allowance(address,address)") + ad(SENDER) + ad(pool)
        checks = [("eth_call", [{"to": t_in, "input": q}, self.tag, ov]),
                  ("eth_call", [{"to": t_in, "input": "0x" + sel("balanceOf(address)") + ad(SENDER)}, self.tag, ov])]
        start = len(self.reqs)
        self.reqs.extend(checks + [("eth_call", [{"from": SENDER, "to": pool, "input": data}, self.tag, ov])])
        self.recs.append((rec, (start, len(checks), dx)))
