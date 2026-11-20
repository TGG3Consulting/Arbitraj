// Файл: contracts/evmtest/ekubo_cases.mjs
// Что делает: решение 458 (T457-01) — сценарии K1–K5 шага EKUBO (25) на официальном коде Ekubo (окружение, источники и
//   список проверок — шапка ekubo.mjs).
// Кто вызывает: contracts/evmtest/run.mjs.
// От чего зависит: ekubo.mjs, lib.mjs, world.mjs.
// Настройки: нет.

import { Interface } from 'ethers';
import * as L from './lib.mjs';
import * as W from './world.mjs';
import * as K from './ekubo.mjs';

const { check, must, view, coder } = L;
const { E18, OWNER, LP, OTHER, WETH, v2, exe, nat, settled, bal } = W;
const { step, quote, coreSwaps, transfers, empty, ZERO } = K;

/// Обмен одним шагом execute и сверка: получено = котировка ядра = выход записи обмена ядра, списано = amountIn.
/// Принимает: окружение, шаг, токены шага, сумму, ключ, вход ключа (нативный — 0), forward-расширение, подпись.
async function one(e, s, tin, tout, amt, key, kin, fwd, name) {
  const { c, ex, core } = e;
  const [owe, q] = await quote(e, key, kin, amt, fwd);
  check(owe === amt && q > 0n, `${name}: котировка ядра — вход ${owe} (= amountIn), выход ${q}`);
  const bad = await L.call(c, OWNER, ex, exe(tin, amt, tout, q + 1n, [s]));
  check(!bad.ok && bad.reason === 'minOut', `${name}: minOut = котировка + 1 — откат «minOut»`);
  const ib = await bal(c, tin, OWNER);
  const ob = await bal(c, tout, OWNER);
  const r = await L.call(c, OWNER, ex, exe(tin, amt, tout, q, [s]));
  check(r.ok, `${name}: обмен прошёл (газ ${r.gas})`);
  const got = (await bal(c, tout, OWNER)) - ob;
  check(got === q, `${name}: владелец получил ровно котировку ядра (${got})`);
  check(ib - (await bal(c, tin, OWNER)) === amt, `${name}: списано ровно amountIn`);
  check(coreSwaps(r, core).length === 1, `${name}: одна запись обмена ядра`);
  check(await empty(e, [tin, tout]), `${name}: на исполнителе пусто (токены и ETH)`);
  return { r, got };
}

export async function ekubo() {
  const record = { note: 'EVM ethereumjs Cancun + CLZ (EIP-7939), contracts/out v4, официальный код Ekubo (broadcast 42161) — contracts/evmtest/ekubo_cases.mjs', cases: [] };
  // K1: ERC-20 ↔ ERC-20.
  {
    const e = await K.env();
    const { core, usdt, coin, kU } = e;
    for (const [tin, tout, dir] of [[usdt, coin, 'USDT → COIN'], [coin, usdt, 'COIN → USDT']]) {
      const s = step(core, tin, tout, kU[2]);
      const o = await one(e, s, tin, tout, 100n * E18, kU, tin, ZERO, `EKUBO ${dir}`);
      const d = coreSwaps(o.r, core)[0];
      const isT1 = BigInt(tin) === BigInt(kU[1]);
      check((isT1 ? -d[0] : -d[1]) === o.got && (isT1 ? d[1] : d[0]) === 100n * E18,
        `EKUBO ${dir}: запись обмена ядра — вход amountIn, выход = полученному`);
      check(transfers(o.r, tout, core, OWNER).length === 1, `EKUBO ${dir}: выход — переводом ядро → владелец (withdraw получателю шага)`);
      record.cases.push({ kind: K.EKUBO, core, token_in: tin, token_out: tout, data: s[5], amount_in: (100n * E18).toString(), got: o.got.toString(), gas: o.r.gas.toString() });
    }
  }
  // K2 / K3: нативная сторона (пул (ETH, COIN), шаг на обёртке WETH9 исполнителя).
  {
    const e = await K.env();
    const { c, ex, core, w, coin, kN } = e;
    await must(c, OWNER, w, WETH.abi, 'deposit', [], 10n * E18);
    await must(c, OWNER, w, WETH.abi, 'approve', [ex, 10n ** 30n]);
    const amt = E18;
    const cb = await L.balance(c, core);
    const o = await one(e, step(core, w, coin, kN[2], true), w, coin, amt, kN, ZERO, ZERO, 'EKUBO нативный вход (execute WETH → COIN)');
    check((await L.balance(c, core)) - cb === amt, 'EKUBO нативный вход: ядру ушло ровно amountIn ETH (withdraw обёртки → value ядру)');
    const [, q] = await quote(e, kN, ZERO, amt);
    const ob = await bal(c, coin, OWNER);
    const sell = await L.call(c, OWNER, ex, nat(true, w, amt, coin, q, [step(core, w, coin, kN[2], true)]), amt);
    const st = settled(sell, ex);
    check(sell.ok && (await bal(c, coin, OWNER)) - ob === q && st && st[0] === amt && st[1] === 0n && await empty(e, [w, coin]),
      `EKUBO executeNative (продажа ETH value): получено ${q} = котировка, NativeSettled(1, 0), на исполнителе пусто`);
    const [, q3] = await quote(e, kN, coin, 100n * E18);
    const wb = await bal(c, w, OWNER);
    const o3 = await one(e, step(core, coin, w, kN[2], true), coin, w, 100n * E18, kN, coin, ZERO, 'EKUBO нативный выход (execute COIN → WETH)');
    check(o3.got === q3 && (await bal(c, w, OWNER)) - wb === q3 && transfers(o3.r, w, ex, OWNER).length === 1,
      'EKUBO нативный выход: ETH ядра → deposit в WETH9 → владельцу ровно котировка');
    const [, q4] = await quote(e, kN, coin, 100n * E18);
    const nb = await L.balance(c, OWNER);
    const buy = await L.call(c, OWNER, ex, nat(false, coin, 100n * E18, w, q4, [step(core, coin, w, kN[2], true)]));
    const sb = settled(buy, ex);
    check(buy.ok && (await L.balance(c, OWNER)) - nb === q4 && sb && sb[1] === q4 && await empty(e, [w, coin]),
      `EKUBO executeNative (покупка ETH): владелец получил ровно ${q4} wei = котировка, на исполнителе пусто`);
    record.cases.push({ kind: K.EKUBO, core, native: true, token_in: w, token_out: coin, amount_in: amt.toString(), got: o.got.toString() });
  }
  // K4: MEVCapture — forward.
  {
    const e = await K.env();
    const { c, ex, core, mev, usdt, coin, kM } = e;
    const amt = 100n * E18;
    const o = await one(e, step(core, usdt, coin, kM[2], false, true), usdt, coin, amt, kM, usdt, mev, 'EKUBO MEVCapture (forward)');
    const d = coreSwaps(o.r, core)[0];
    const pool = BigInt(usdt) === BigInt(kM[0]) ? -d[1] : -d[0];
    check(o.got < pool, `EKUBO MEVCapture: получено ${o.got} < выхода пула ${pool} — доплата MEVCapture учтена (balanceUpdate forward)`);
    const nf = await L.call(c, OWNER, ex, exe(usdt, amt, coin, 0n, [step(core, usdt, coin, kM[2], false, false)]));
    check(!nf.ok && nf.ret.startsWith(new Interface(['error SwapMustHappenThroughForward()']).getError('SwapMustHappenThroughForward').selector),
      'EKUBO пул MEVCapture без forward — откат SwapMustHappenThroughForward (MEVCapture.sol L84-L86)');
    record.cases.push({ kind: K.EKUBO, core, forward: mev, token_in: usdt, token_out: coin, amount_in: amt.toString(), got: o.got.toString() });
  }
  // K5: защита.
  {
    const e = await K.env();
    const { c, ex, core, usdt, coin, pairCU, kU } = e;
    const X = new Interface(v2.abi);
    const data = X.encodeFunctionData('locked_6416899205', [0n]) + coder.encode(['address', 'address', 'uint256', 'address', 'bytes32', 'bool', 'bool'], [usdt, coin, E18, OTHER, kU[2], false, false]).slice(2);
    for (const [who, name] of [[OTHER, 'посторонний'], [OWNER, 'владелец вне шага'], [core, 'ядро вне шага']]) {
      const r = await L.call(c, who, ex, data);
      check(!r.ok && r.reason === 'pool', `${name} зовёт locked_6416899205 — откат «pool»`);
    }
    const other = '0x' + ((BigInt(kU[2]) & ~0xffffffffn) | 0x80000000n | 20000n).toString(16).padStart(64, '0');
    const np = await L.call(c, OWNER, ex, exe(usdt, E18, coin, 0n, [step(core, usdt, coin, other)]));
    check(!np.ok && np.ret.slice(0, 10) === '0x486aa307', 'EKUBO: пул с другим config не создан — откат ядра PoolNotInitialized() (0x486aa307)');
    const nc = await L.call(c, OWNER, ex, exe(usdt, E18, coin, 0n, [step(pairCU, usdt, coin, kU[2])]));
    // У PancakePair нет lock() и нет fallback — откат без данных; _ekuboCall пробрасывает его как есть.
    check(!nc.ok && nc.ret === '0x', 'EKUBO: «ядро» шага — пара V2 (нет lock) — откат без данных');
    const nw = await L.call(c, OWNER, ex, exe(usdt, E18, coin, 0n, [step(core, usdt, coin, kU[2], true)]));
    check(!nw.ok && nw.reason === 'wnative', 'EKUBO: native у шага без обёртки исполнителя — откат «wnative»');
    const tax = await L.deploy(c, LP, K.TAX, ['uint256'], [10n ** 30n]);
    await must(c, LP, tax, K.TAX.abi, 'transfer', [OWNER, 1_000n * E18]);
    await must(c, LP, tax, K.TAX.abi, 'approve', [e.pos, 10n ** 30n]);
    const kt = [...K.sort(tax, coin), K.config()];
    await K.addLiquidity(e, kt);
    await must(c, OWNER, tax, K.TAX.abi, 'approve', [ex, 10n ** 30n]);
    const tb = BigInt(await view(c, tax, K.TAX.abi, 'balanceOf', [OWNER]));
    const tr = await L.call(c, OWNER, ex, exe(tax, 10n * E18, coin, 0n, [step(core, tax, coin, kt[2])]));
    check(!tr.ok && tr.ret.slice(0, 10) === '0x9731ba37' && BigInt(await view(c, tax, K.TAX.abi, 'balanceOf', [OWNER])) === tb,
      'EKUBO: токен с налогом на входе — откат всего (DebtsNotZeroed 0x9731ba37, FlashAccountant.sol L171-L177: ядро недосчиталось), владелец при своих');
  }
  {
    const e = await K.env(W.v3f);
    const r = await L.call(e.c, OWNER, e.ex, exe(e.usdt, E18, e.coin, 0n, [step(e.core, e.usdt, e.coin, e.kU[2])]));
    check(!r.ok && r.reason === 'kind', 'исполнитель v3 (out/v3): шаг EKUBO (25) — откат «kind»');
  }
  return record;
}
