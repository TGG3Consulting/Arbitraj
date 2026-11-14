// Файл: contracts/evmtest/review.mjs
// Что делает: сценарии ревью 272 (А4) EVM-теста ArbExecutor v2 — те, что ревьюер прогонял вручную:
//   R1) настоящий повторный вход ОТ ВЛАДЕЛЬЦА (владелец — контракт-фикстура OwnerProxy; токен из transfer зовёт
//       владельца, тот — исполнитель): внутренний executeNative и execute откатываются именно замком «lock»;
//   R2) посторонний зовёт каждый обратный вызов (V3, Pancake V3, Algebra, Elastic, Maverick) — откат;
//   R3) злой пул V3 внутри executeNative: обратный вызов дважды («pool»), долг больше суммы шага («owed»), взял и не
//       отдал («minOut») — в обе стороны, балансы не меняются;
//   R4) шаги V3 (честный пул-фикстура) в executeNative: продажа, покупка, доля с остатком;
//   R5) поддельная обёртка (deposit принимает, при withdraw BNB не отдаёт): покупка — «minOut», продажа с остатком —
//       NativeSettled(value, 0) (вернулось 0 — честно);
//   R6) подкинутый BNB (баланс без вызова, как selfdestruct) и подкинутый WBNB: событие и «получено» — только свои
//       операции, постороннее уходит владельцу; в т.ч. шаг STABLE_U256_BNB с выходом BNB;
//   R7) правило «native step» (ревью А2): [USDT→WBNB, WBNB→USDT] и продажи, возвращающие обёртку / тратящие выход — откат;
//   R8) многошаговые маршруты и доли (продажа двумя долями, покупка через промежуточную монету, покупка двумя долями);
//   R9) deadline при заданном времени блока.
// Кто вызывает: contracts/evmtest/run.mjs (review).
// От чего зависит: lib.mjs, world.mjs, fixtures/{OwnerProxy, EvilV3Pool, FakeWrapper, ReentrantToken}.sol.
// Настройки: нет.

import { Interface } from 'ethers';
import * as L from './lib.mjs';
import * as W from './world.mjs';

const { check, coder, must, view } = L;
const { E18, OWNER, LP, OTHER, ERC20, FACT, WETH, RE, OWN, EVIL, FAKE, X, v2, world, pairOf, v2Step, v3Step, stStep, nat, exe, settled, bal, v2Out } = W;
const MAX = 10n ** 30n;
const reason = (hex) => (hex && hex.startsWith('0x08c379a0') ? coder.decode(['string'], '0x' + hex.slice(10))[0] : hex);

/// Все сценарии ревью. Принимает: запись для тестов бота (дополняется). Ошибки: исключение «НЕ ПРОШЛО: …».
export async function review(record) {
  // R1) Повторный вход от владельца.
  {
    const { c, f, w } = await world();
    const P = await L.deploy(c, OWNER, OWN);
    const IP = new Interface(OWN.abi);
    const dr = await must(c, OWNER, P, OWN.abi, 'deploy', ['0x' + v2.bin + coder.encode(['address'], [w]).slice(2)]);
    const ex = IP.decodeFunctionResult('deploy', dr.ret)[0];
    check((await view(c, ex, v2.abi, 'owner')).toLowerCase() === P.toLowerCase(), 'R1: владелец исполнителя — контракт OwnerProxy');
    const t = await L.deploy(c, LP, RE, ['uint256'], [MAX]);
    const pair = await pairOf(c, f, t, w, 1000n * E18, 10n * E18, RE.abi, WETH.abi);
    await must(c, LP, t, RE.abi, 'transfer', [P, 100n * E18]);
    await must(c, OWNER, P, OWN.abi, 'run', [t, new Interface(RE.abi).encodeFunctionData('approve', [ex, MAX])]);
    const via = (d) => IP.encodeFunctionData('run', [ex, d]);
    for (const [name, inner] of [['executeNative', nat(false, t, 1n, w, 1n, [v2Step(pair, t, w)])], ['execute', exe(t, 1n, w, 0n, [v2Step(pair, t, w)])]]) {
      await must(c, OWNER, t, RE.abi, 'armSwallow', [P, via(inner)]);
      const r = await L.call(c, OWNER, P, via(nat(false, t, E18, w, 1n, [v2Step(pair, t, w)])));
      const inn = reason(await view(c, t, RE.abi, 'lastRet'));
      check(r.ok && inn === 'lock', `R1: повторный ${name} от владельца из transfer токена — откат замком «lock» (внешний вызов прошёл)`);
    }
  }
  // R2) Посторонний зовёт обратные вызовы.
  {
    const { c, usdt, ex } = await world();
    const cbs = [['uniswapV3SwapCallback', [1, 1, '0x']], ['pancakeV3SwapCallback', [1, 1, '0x']], ['algebraSwapCallback', [1, 1, '0x']],
      ['swapCallback', [1, 1, '0x']], ['maverickV2SwapCallback', [usdt, 1, 0, '0x']], ['maverickV2SwapCallback', ['0x' + '00'.repeat(20), 1, 0, '0x']]];
    for (const [n, a] of cbs) {
      const r = await L.call(c, OTHER, ex, X.encodeFunctionData(n, a));
      check(!r.ok && ['pool', 'token'].includes(r.reason), `R2: посторонний зовёт ${n} — откат «${r.reason}»`);
    }
  }
  // R3) Злой пул V3 и R4) честный пул V3.
  const v3world = async (mode) => {
    const wd = await world();
    const pool = await L.deploy(wd.c, LP, EVIL, ['address', 'address', 'uint8'], [wd.w, wd.usdt, mode]);
    await must(wd.c, LP, wd.w, WETH.abi, 'transfer', [pool, 100n * E18]);
    await must(wd.c, LP, wd.usdt, ERC20.abi, 'transfer', [pool, 1000n * E18]);
    await must(wd.c, OWNER, wd.usdt, ERC20.abi, 'approve', [wd.ex, MAX]);
    return { ...wd, pool };
  };
  for (const [mode, want, what] of [[1, 'pool', 'обратный вызов дважды'], [2, 'owed', 'долг больше суммы шага'], [3, 'minOut', 'взял и не отдал']]) {
    const { c, w, usdt, ex, pool } = await v3world(mode);
    const [nb, ub] = [await L.balance(c, OWNER), await bal(c, usdt, OWNER)];
    const s = await L.call(c, OWNER, ex, nat(true, w, E18, usdt, 1n, [v3Step(pool, w, usdt)]), E18);
    const b = await L.call(c, OWNER, ex, nat(false, usdt, E18, w, 1n, [v3Step(pool, usdt, w)]));
    const same = (await L.balance(c, OWNER)) === nb && (await bal(c, usdt, OWNER)) === ub;
    check(!s.ok && s.reason === want && !b.ok && b.reason === want && same,
      `R3: злой пул V3 (${what}) в executeNative — продажа и покупка откатываются «${want}», балансы владельца те же`);
  }
  {
    const { c, w, usdt, ex, pool } = await v3world(0);
    const ub = await bal(c, usdt, OWNER);
    const s = await L.call(c, OWNER, ex, nat(true, w, E18, usdt, E18, [v3Step(pool, w, usdt)]), E18);
    const ss = settled(s, ex);
    check(s.ok && (await bal(c, usdt, OWNER)) - ub === E18 && ss[0] === E18 && ss[1] === 0n, 'R4: продажа BNB шагом V3 — USDT владельцу, NativeSettled(1, 0)');
    const nb = await L.balance(c, OWNER);
    const b = await L.call(c, OWNER, ex, nat(false, usdt, E18, w, E18, [v3Step(pool, usdt, w)]));
    const sb = settled(b, ex);
    check(b.ok && (await L.balance(c, OWNER)) - nb === E18 && sb[1] === E18, 'R4: покупка BNB шагом V3 — BNB владельцу, NativeSettled(0, 1)');
    const h = await L.call(c, OWNER, ex, nat(true, w, E18, usdt, 1n, [v3Step(pool, w, usdt, 5000)]), E18);
    const sh = settled(h, ex);
    check(h.ok && sh[1] === E18 / 2n && (await bal(c, w, ex)) === 0n, 'R4: доля V3 5000 bps — остаток 0,5 вернулся нативным, на исполнителе пусто');
  }
  // R5) Поддельная обёртка.
  {
    const c = await L.chain();
    for (const a of [OWNER, LP]) await L.fund(c, a, 10_000n * E18);
    const fake = await L.deploy(c, LP, FAKE);
    const usdt = await L.deploy(c, LP, ERC20, ['uint256'], [MAX]);
    const f = await L.deploy(c, LP, FACT, ['address'], [LP]);
    await must(c, LP, fake, FAKE.abi, 'deposit', [], 100n * E18);
    const pair = await pairOf(c, f, usdt, fake, 60_000n * E18, 100n * E18, ERC20.abi, FAKE.abi);
    await must(c, LP, usdt, ERC20.abi, 'transfer', [OWNER, 10_000n * E18]);
    const ex = await W.executor(c, OWNER, v2, fake);
    await must(c, OWNER, usdt, ERC20.abi, 'approve', [ex, MAX]);
    const b = await L.call(c, OWNER, ex, nat(false, usdt, 1000n * E18, fake, 1n, [v2Step(pair, usdt, fake)]));
    check(!b.ok && b.reason === 'minOut', 'R5: поддельная обёртка (withdraw без BNB) — покупка откатывается «minOut» (получено 0)');
    const s = await L.call(c, OWNER, ex, nat(true, fake, E18, usdt, 1n, [v2Step(pair, fake, usdt, 5000)]), E18);
    const ss = settled(s, ex);
    check(s.ok && ss[0] === E18 && ss[1] === 0n, 'R5: поддельная обёртка — продажа с остатком: NativeSettled(1, 0) — вернулось 0, как в сети');
  }
  // R6) Подкинутый BNB и WBNB.
  {
    const { c, w, usdt, pair, ex } = await world();
    await L.setBalance(c, ex, 3n * E18);
    const nb = await L.balance(c, OWNER);
    const r = await L.call(c, OWNER, ex, nat(true, w, E18, usdt, 1n, [v2Step(pair, w, usdt, 5000)]), E18);
    const s = settled(r, ex);
    check(r.ok && s[0] === E18 && s[1] === E18 / 2n, 'R6: продажа с остатком при подкинутых 3 BNB — NativeSettled(1, 0,5): подкинутое не в событии');
    check((await L.balance(c, OWNER)) - nb === 5n * E18 / 2n && (await L.balance(c, ex)) === 0n,
      'R6: подкинутые 3 BNB ушли владельцу (−1 + 0,5 + 3), на исполнителе 0');
    record.cases.sell_foreign_bnb = { executor: ex, owner: OWNER, wbnb: w, usdt, value: E18.toString(), returned: (E18 / 2n).toString(), foreign_wei: (3n * E18).toString(), logs: r.logs };
  }
  {
    const { c, w, usdt, pair, ex } = await world();
    await must(c, OWNER, usdt, ERC20.abi, 'approve', [ex, MAX]);
    const amt = 1000n * E18;
    const exp = await v2Out(c, pair, usdt, amt);
    await L.setBalance(c, ex, 2n * E18);
    await must(c, LP, w, WETH.abi, 'transfer', [ex, E18]);
    const [nb, wb] = [await L.balance(c, OWNER), await bal(c, w, OWNER)];
    const r = await L.call(c, OWNER, ex, nat(false, usdt, amt, w, exp, [v2Step(pair, usdt, w)]));
    const s = settled(r, ex);
    const got = r.ok ? BigInt(coder.decode(['uint256'], r.ret)[0]) : -1n;
    check(r.ok && s[1] === exp && got === exp, 'R6: покупка при подкинутых 2 BNB и 1 WBNB — NativeSettled и «получено» = выход пары, без подкинутого');
    check((await L.balance(c, OWNER)) - nb === exp + 2n * E18 && (await bal(c, w, OWNER)) - wb === E18,
      'R6: подкинутые BNB и WBNB ушли владельцу (BNB — нативным, WBNB — токеном)');
  }
  {
    const { c, w, coin, stable, ex } = await world();
    await must(c, OWNER, coin, ERC20.abi, 'approve', [ex, MAX]);
    await L.setBalance(c, ex, 5n * E18);
    const nb = await L.balance(c, OWNER);
    const r = await L.call(c, OWNER, ex, nat(false, coin, E18, w, E18, [stStep(stable, coin, w, 1, 0, false)]));
    const s = settled(r, ex);
    check(r.ok && s[1] === E18 && (await L.balance(c, OWNER)) - nb === 6n * E18,
      'R6: STABLE_U256_BNB (BNB от пула) при подкинутых 5 BNB — в обёртку и событие только BNB пула; подкинутое владельцу');
  }
  // R7) Правило «native step».
  {
    const { c, w, usdt, coin, pair, pairCW, pairCU, ex } = await world();
    await must(c, OWNER, usdt, ERC20.abi, 'approve', [ex, MAX]);
    const routes = [
      [false, 'покупка [USDT→WBNB, WBNB→USDT]', nat(false, usdt, 1000n * E18, w, 1n, [v2Step(pair, usdt, w), v2Step(pair, w, usdt)]), 0n],
      [true, 'продажа с шагом, отдающим обёртку', nat(true, w, E18, usdt, 1n, [v2Step(pairCW, w, coin, 5000), v2Step(pairCW, coin, w), v2Step(pair, w, usdt)]), E18],
      [true, 'продажа с шагом, тратящим выход', nat(true, w, E18, usdt, 1n, [v2Step(pair, w, usdt, 5000), v2Step(pairCU, usdt, coin)]), E18],
    ];
    for (const [, what, data, value] of routes) {
      const r = await L.call(c, OWNER, ex, data, value);
      check(!r.ok && r.reason === 'native step', `R7: ${what} — откат «native step»`);
    }
  }
  // R8) Многошаговые маршруты и доли.
  {
    const { c, w, usdt, coin, pair, pairCW, pairCU, ex } = await world();
    const half = E18 / 2n;
    const exp = (await v2Out(c, pair, w, half)) + (await v2Out(c, pairCU, coin, await v2Out(c, pairCW, w, half)));
    const ub = await bal(c, usdt, OWNER);
    const r = await L.call(c, OWNER, ex, nat(true, w, E18, usdt, exp, [v2Step(pair, w, usdt, 5000), v2Step(pairCW, w, coin), v2Step(pairCU, coin, usdt)]), E18);
    const s = settled(r, ex);
    const empty = (await bal(c, w, ex)) + (await bal(c, coin, ex)) + (await bal(c, usdt, ex)) === 0n;
    check(r.ok && (await bal(c, usdt, OWNER)) - ub === exp && s[0] === E18 && s[1] === 0n && empty,
      'R8: продажа BNB двумя долями (прямо и через COIN) — USDT = сумма выходов, NativeSettled(1, 0), на исполнителе пусто');
  }
  for (const split of [false, true]) {
    const { c, w, usdt, coin, pair, pairCW, pairCU, ex } = await world();
    await must(c, OWNER, usdt, ERC20.abi, 'approve', [ex, MAX]);
    const amt = 1000n * E18;
    const part = split ? amt / 2n : amt;
    const viaCoin = await v2Out(c, pairCW, coin, await v2Out(c, pairCU, usdt, part));
    const exp = split ? (await v2Out(c, pair, usdt, part)) + viaCoin : viaCoin;
    const steps = [v2Step(pairCU, usdt, coin), v2Step(pairCW, coin, w)];
    if (split) steps.unshift(v2Step(pair, usdt, w, 5000));
    const nb = await L.balance(c, OWNER);
    const r = await L.call(c, OWNER, ex, nat(false, usdt, amt, w, exp, steps));
    const s = settled(r, ex);
    check(r.ok && (await L.balance(c, OWNER)) - nb === exp && s[1] === exp,
      `R8: покупка BNB ${split ? 'двумя долями (прямо и через COIN)' : 'через COIN (прямой вход первого шага)'} — BNB = выход маршрута, NativeSettled(0, выход)`);
  }
  // R9) deadline.
  {
    const { c, w, usdt, pair, ex } = await world();
    c.ts = 1000n;
    await must(c, OWNER, usdt, ERC20.abi, 'approve', [ex, MAX]);
    const late = await L.call(c, OWNER, ex, nat(true, w, E18, usdt, 1n, [v2Step(pair, w, usdt)], 999n), E18);
    const lateE = await L.call(c, OWNER, ex, exe(usdt, E18, w, 1n, [v2Step(pair, usdt, w)], 999n));
    check(!late.ok && late.reason === 'deadline' && !lateE.ok && lateE.reason === 'deadline',
      'R9: время блока 1000, срок 999 — executeNative и execute откатываются «deadline»');
    const ok = await L.call(c, OWNER, ex, nat(true, w, E18, usdt, 1n, [v2Step(pair, w, usdt)], 1000n), E18);
    check(ok.ok, 'R9: срок = времени блока — проходит');
  }
}
