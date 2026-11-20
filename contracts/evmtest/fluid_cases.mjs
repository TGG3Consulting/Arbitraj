// Файл: contracts/evmtest/fluid_cases.mjs
// Что делает: решение 458 (T457-01) — сценарии шага FLUID (22) исполнителя ArbExecutor v4 на официальном коде Fluid DEX T1
//   (окружение и источники — fluid.mjs). Котировка — сам пул: swapIn(…, to = 0x…dEaD) откатывается с
//   FluidDexSwapResult(выход) (poolT1 coreModule/core/main.sol L47, L338 @404d73f — путь оценки резолвера).
//   F1) ERC-20 (USDT/COIN) обе стороны: владелец получил ровно котировку пула и выход события Swap (to = владелец),
//       списано ровно amountIn, на исполнителе пусто, разрешение пулу после шага 0; minOut + 1 — «minOut»;
//   F2) нативный вход (пул COIN / ETH-метка 0xEeee): шаг WETH → COIN, native = 1 — WETH withdraw → swapIn{value}:
//       execute (WETH владельца) и executeNative (продажа ETH value): получено = котировка, слой ликвидности получил ровно
//       amountIn ETH, на исполнителе 0 ETH / WETH;
//   F3) нативный выход: COIN → WETH, native = 2 — ETH от слоя ликвидности (receive пускает только liquidity из data,
//       bnbFrom) → deposit в WETH9 → владельцу ровно котировка; executeNative (покупка ETH) — ETH владельцу = котировка;
//   F4) два шага: USDT →(FLUID)→ COIN →(FLUID, нативный выход)→ WETH одной транзакцией;
//   F5) защита: в data чужой адрес слоя ликвидности при нативном выходе — ETH от настоящего слоя отвергнут, откат всего;
//       native = 3 — «native»; «пул» шага — пара V2 — откат; направление не по токенам шага — откат; исполнитель v3 —
//       «kind».
// Кто вызывает: contracts/evmtest/run.mjs.
// От чего зависит: fluid.mjs, lib.mjs, world.mjs.
// Настройки: нет.

import * as L from './lib.mjs';
import * as W from './world.mjs';
import * as F from './fluid.mjs';

const { check, must, coder } = L;
const { E18, OWNER, OTHER, ERC20, WETH, exe, nat, settled, bal } = W;
const FLUID = 22;
const step = (pool, tin, tout, zf, native, liq, bps = 10000) =>
  [FLUID, pool, tin, tout, bps, coder.encode(['bool', 'uint8', 'address'], [zf, native, liq])];

/// Котировка пула: swapIn(zf, amt, 0, dEaD) от OTHER (нативный вход — с value) → FluidDexSwapResult(выход).
async function quote(c, pool, zf, amt, value = 0n) {
  const r = await L.call(c, OTHER, pool, F.POOL.encodeFunctionData('swapIn', [zf, amt, 0n, F.DEAD]), value);
  const e = F.POOL.parseError(r.ret);
  if (r.ok || e?.name !== 'FluidDexSwapResult') throw new Error(`quote: ${r.reason} ${r.ret.slice(0, 74)}`);
  return BigInt(e.args[0]);
}

/// События Swap пула: [{ swap0to1, amountIn, amountOut, to }].
const swaps = (r, pool) => r.logs.filter((l) => l.address.toLowerCase() === pool.toLowerCase()
  && l.topics[0] === F.POOL.getEvent('Swap').topicHash).map((l) => F.POOL.decodeEventLog('Swap', l.data, l.topics));

const emptyEx = async (c, ex, ts) => {
  for (const t of ts) if ((await bal(c, t, ex)) !== 0n) return false;
  return (await L.balance(c, ex)) === 0n;
};

export async function fluid() {
  const record = { note: 'EVM ethereumjs Cancun, contracts/out v4, официальный Fluid DEX T1 (fluid-contracts-public@404d73f deployments/arbitrum) — contracts/evmtest/fluid_cases.mjs', cases: [] };
  // F1.
  {
    const e = await F.env();
    const { c, ex, pU, t0U, t1U, liq } = e;
    const amt = 100n * E18;
    for (const [tin, tout] of [[t0U, t1U], [t1U, t0U]]) {
      const zf = tin === t0U;
      const dir = `${tin === e.usdt ? 'USDT' : 'COIN'} → ${tout === e.usdt ? 'USDT' : 'COIN'}`;
      const q = await quote(c, pU, zf, amt);
      const s = step(pU, tin, tout, zf, 0, liq);
      const bad = await L.call(c, OWNER, ex, exe(tin, amt, tout, q + 1n, [s]));
      check(!bad.ok && bad.reason === 'minOut', `FLUID ${dir}: minOut = котировка пула + 1 — откат «minOut»`);
      const ib = await bal(c, tin, OWNER);
      const ob = await bal(c, tout, OWNER);
      const r = await L.call(c, OWNER, ex, exe(tin, amt, tout, q, [s]));
      check(r.ok, `FLUID ${dir}: обмен прошёл (газ ${r.gas})`);
      const got = (await bal(c, tout, OWNER)) - ob;
      const ev = swaps(r, pU);
      check(q > 0n && got === q && ev.length === 1 && BigInt(ev[0].amountOut) === q && BigInt(ev[0].to) === BigInt(OWNER),
        `FLUID ${dir}: владелец получил ровно котировку пула и выход события Swap (to = владелец, ${got})`);
      check(ib - (await bal(c, tin, OWNER)) === amt && BigInt(ev[0].amountIn) === amt, `FLUID ${dir}: списано ровно amountIn, пул взял ровно его`);
      check(await emptyEx(c, ex, [tin, tout]) && BigInt(await L.view(c, tin, ERC20.abi, 'allowance', [ex, pU])) === 0n,
        `FLUID ${dir}: на исполнителе пусто, разрешение пулу после шага 0`);
      record.cases.push({ kind: FLUID, pool: pU, token_in: tin, token_out: tout, data: s[5], amount_in: amt.toString(), got: got.toString(), gas: r.gas.toString() });
    }
  }
  // F2 / F3: нативная сторона.
  {
    const e = await F.env();
    const { c, ex, pN, coin, w, liq } = e;
    await must(c, OWNER, w, WETH.abi, 'deposit', [], 10n * E18);
    await must(c, OWNER, w, WETH.abi, 'approve', [ex, 10n ** 30n]);
    const amt = E18;
    const q = await quote(c, pN, false, amt, amt);
    const lb = await L.balance(c, liq);
    const ob = await bal(c, coin, OWNER);
    const r = await L.call(c, OWNER, ex, exe(w, amt, coin, q, [step(pN, w, coin, false, 1, liq)]));
    check(r.ok && (await bal(c, coin, OWNER)) - ob === q && (await L.balance(c, liq)) - lb === amt && await emptyEx(c, ex, [w, coin]),
      `FLUID нативный вход (execute WETH → COIN): получено ${q} = котировка, слою ликвидности ровно ${amt} ETH, на исполнителе пусто (газ ${r.gas})`);
    const q2 = await quote(c, pN, false, amt, amt);
    const ob2 = await bal(c, coin, OWNER);
    const sell = await L.call(c, OWNER, ex, nat(true, w, amt, coin, q2, [step(pN, w, coin, false, 1, liq)]), amt);
    const st = settled(sell, ex);
    check(sell.ok && (await bal(c, coin, OWNER)) - ob2 === q2 && st && st[0] === amt && st[1] === 0n && await emptyEx(c, ex, [w, coin]),
      `FLUID executeNative (продажа ETH value): получено ${q2} = котировка, NativeSettled(1, 0), на исполнителе пусто`);
    const a3 = 100n * E18;
    const q3 = await quote(c, pN, true, a3);
    const bad = await L.call(c, OWNER, ex, exe(coin, a3, w, q3 + 1n, [step(pN, coin, w, true, 2, liq)]));
    check(!bad.ok && bad.reason === 'minOut', 'FLUID нативный выход: minOut = котировка + 1 — откат «minOut»');
    const wb = await bal(c, w, OWNER);
    const r3 = await L.call(c, OWNER, ex, exe(coin, a3, w, q3, [step(pN, coin, w, true, 2, liq)]));
    check(r3.ok && (await bal(c, w, OWNER)) - wb === q3 && await emptyEx(c, ex, [w, coin])
      && BigInt(await L.view(c, coin, ERC20.abi, 'allowance', [ex, pN])) === 0n,
      `FLUID нативный выход (execute COIN → WETH): ETH слоя ликвидности → deposit → владельцу ровно котировка ${q3}, на исполнителе пусто (газ ${r3.gas})`);
    const q4 = await quote(c, pN, true, a3);
    const nb = await L.balance(c, OWNER);
    const buy = await L.call(c, OWNER, ex, nat(false, coin, a3, w, q4, [step(pN, coin, w, true, 2, liq)]));
    const sb = settled(buy, ex);
    check(buy.ok && (await L.balance(c, OWNER)) - nb === q4 && sb && sb[1] === q4 && await emptyEx(c, ex, [w, coin]),
      `FLUID executeNative (покупка ETH): владелец получил ровно ${q4} wei = котировка, на исполнителе пусто`);
    // F5 (часть): чужой адрес слоя ликвидности в data при нативном выходе.
    const ob5 = await bal(c, coin, OWNER);
    const wl = await L.call(c, OWNER, ex, exe(coin, a3, w, 0n, [step(pN, coin, w, true, 2, OTHER)]));
    const we = (() => { try { return F.POOL.parseError(wl.ret)?.name; } catch { return null; } })();
    check(!wl.ok && we === 'FluidSafeTransferError' && (await bal(c, coin, OWNER)) === ob5,
      `FLUID нативный выход с чужим адресом слоя ликвидности в data — ETH от слоя отвергнут receive, откат всего (${we} — перевод ETH слоем ликвидности не прошёл), владелец при своих`);
    const n3 = await L.call(c, OWNER, ex, exe(coin, a3, w, 0n, [step(pN, coin, w, true, 3, liq)]));
    check(!n3.ok && n3.reason === 'native', 'FLUID native = 3 — откат «native»');
    record.cases.push({ kind: FLUID, pool: pN, native_in: true, token_in: w, token_out: coin, amount_in: amt.toString(), got: q.toString() });
    record.cases.push({ kind: FLUID, pool: pN, native_out: true, token_in: coin, token_out: w, amount_in: a3.toString(), got: q3.toString() });
  }
  // F4 + F5.
  {
    const e = await F.env();
    const { c, ex, pU, pN, usdt, coin, w, liq, t0U, pairCU } = e;
    const wb = await bal(c, w, OWNER);
    const zfU = usdt === t0U;
    const r = await L.call(c, OWNER, ex, exe(usdt, 100n * E18, w, 1n, [step(pU, usdt, coin, zfU, 0, liq), step(pN, coin, w, true, 2, liq)]));
    const s1 = swaps(r, pU);
    const s2 = swaps(r, pN);
    check(r.ok && s1.length === 1 && s2.length === 1 && BigInt(s2[0].amountIn) === BigInt(s1[0].amountOut)
      && (await bal(c, w, OWNER)) - wb === BigInt(s2[0].amountOut) && await emptyEx(c, ex, [usdt, coin, w]),
      `FLUID двумя шагами USDT → COIN → WETH (нативный выход): вход второго = выход первого, владелец получил выход второго, на исполнителе пусто (газ ${r.gas})`);
    const np = await L.call(c, OWNER, ex, exe(usdt, E18, coin, 0n, [step(pairCU, usdt, coin, zfU, 0, liq)]));
    check(!np.ok, `FLUID: «пул» шага — пара V2 — откат (${np.reason})`);
    const wd = await L.call(c, OWNER, ex, exe(usdt, E18, coin, 0n, [step(pU, usdt, coin, !zfU, 0, liq)]));
    check(!wd.ok, `FLUID: направление не по токенам шага — откат (${wd.reason ?? wd.ret.slice(0, 10)})`);
  }
  {
    const e = await F.env(W.v3f);
    const r = await L.call(e.c, OWNER, e.ex, exe(e.t0U, E18, e.t1U, 0n, [step(e.pU, e.t0U, e.t1U, true, 0, e.liq)]));
    check(!r.ok && r.reason === 'kind', 'исполнитель v3 (out/v3): шаг FLUID (22) — откат «kind»');
  }
  return record;
}
