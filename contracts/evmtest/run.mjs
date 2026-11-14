// Файл: contracts/evmtest/run.mjs
// Что делает: EVM-тест исполнителя ArbExecutor v2 (решение 272) на EVM ethereumjs (Cancun): реальные PancakePair /
//   PancakeFactory / ERC20 (pancake-swap-core@3b214306) и WETH9 (v2-periphery@ed249913) — обёртка с withdraw через
//   transfer (стипендия 2300 газа); фикстуры только там, где официального кода в репозитории нет (пул StableSwap с BNB,
//   токен с повторным входом — fixtures/). Проверяется:
//   1) код из contracts/out/ — ровно результат компиляции contracts/ArbExecutor.sol (те же настройки, что build.mjs);
//   2) продажа BNB executeNative(wrapIn = true): value → deposit → пара V2 → USDT владельцу; NativeSettled(value, 0);
//   3) продажа с недоеденным остатком (доля 5000 bps): остаток развёрнут и вернулся нативным BNB, NativeSettled(value, остаток);
//   4) покупка BNB executeNative(wrapIn = false): USDT → пара (прямой вход) → WBNB исполнителю → withdraw → BNB владельцу;
//   5) откат по minOut в обе стороны (балансы не меняются); 6) неверный wnative (адрес без кода, ERC-20 без deposit);
//   7) не владелец; 8) повторный вход из токена; 9) шаг STABLE_U256_BNB внутри обоих направлений, bnb_gas пула 2300;
//   10) BNB напрямую на исполнитель — откат «bnb»; 11) execute (ERC-20) v2 — как раньше; 12) исполнитель v1
//   (contracts/out/v1/) — execute работает, executeNative у него нет (откат).
// Кто вызывает: человек или агент: `cd contracts/evmtest && npm ci && node run.mjs` (README.md папки).
// От чего зависит: lib.mjs, contracts/*, vendor/*, fixtures/*.
// Настройки: нет.

import { join } from 'node:path';
import { Interface } from 'ethers';
import * as L from './lib.mjs';

const { check, coder, must, view } = L;
const E18 = 10n ** 18n;
const OWNER = '0x' + 'a1'.repeat(20);
const LP = '0x' + 'b2'.repeat(20);
const OTHER = '0x' + 'c3'.repeat(20);
const FAR = 2n ** 40n;

// 1) Сборка и сверка с out/.
const opt = { optimizer: { enabled: true, runs: 200 }, evmVersion: 'cancun' };
const v2 = L.artifact('out');
const v1 = L.artifact('out/v1');
const built = L.compile(L.solc08, join(L.here, '..'), 'ArbExecutor.sol', 'ArbExecutor', opt);
check(built.runtime === v2.runtime && built.bin === v2.bin, 'contracts/out/ArbExecutor.* = компиляция ArbExecutor.sol');
const pc = join(L.here, 'vendor', 'pancake-swap-core');
const ERC20 = L.compile(L.solc05, pc, 'test/ERC20.sol', 'ERC20', { optimizer: { enabled: true, runs: 999999 } });
const FACT = L.compile(L.solc05, pc, 'PancakeFactory.sol', 'PancakeFactory', { optimizer: { enabled: true, runs: 999999 } });
const PAIR = L.compile(L.solc05, pc, 'PancakePair.sol', 'PancakePair', { optimizer: { enabled: true, runs: 999999 } });
const WETH = L.compile(L.solc06, join(L.here, 'vendor', 'v2-periphery'), 'WETH9.sol', 'WETH9', { optimizer: { enabled: true, runs: 999999 } });
const STB = L.compile(L.solc08, join(L.here, 'fixtures'), 'StableBnbFixture.sol', 'StableBnbFixture', opt);
const RE = L.compile(L.solc08, join(L.here, 'fixtures'), 'ReentrantToken.sol', 'ReentrantToken', opt);
const X = new Interface(v2.abi);

/// Цепь с владельцем, WBNB, USDT, монетой COIN, парой USDT/WBNB и пулом-фикстурой BNB/COIN.
async function world(art = v2) {
  const c = await L.chain();
  for (const a of [OWNER, LP, OTHER]) await L.fund(c, a, 10_000n * E18);
  const w = await L.deploy(c, LP, WETH);
  const usdt = await L.deploy(c, LP, ERC20, ['uint256'], [10n ** 30n]);
  const coin = await L.deploy(c, LP, ERC20, ['uint256'], [10n ** 30n]);
  const f = await L.deploy(c, LP, FACT, ['address'], [LP]);
  await must(c, LP, f, FACT.abi, 'createPair', [usdt, w]);
  const pair = await view(c, f, FACT.abi, 'getPair', [usdt, w]);
  await must(c, LP, w, WETH.abi, 'deposit', [], 100n * E18);
  await must(c, LP, w, WETH.abi, 'transfer', [pair, 100n * E18]);
  await must(c, LP, usdt, ERC20.abi, 'transfer', [pair, 60_000n * E18]);
  await must(c, LP, pair, PAIR.abi, 'mint', [LP]);
  const stable = await L.deploy(c, LP, STB, ['address', 'uint256'], [coin, 2300n], 50n * E18);
  await must(c, LP, coin, ERC20.abi, 'transfer', [stable, 50n * E18]);
  await must(c, LP, usdt, ERC20.abi, 'transfer', [OWNER, 10_000n * E18]);
  await must(c, LP, coin, ERC20.abi, 'transfer', [OWNER, 10n * E18]);
  const ex = await L.deploy(c, OWNER, art);
  return { c, w, usdt, coin, pair, stable, ex };
}

const v2Step = (pool, tin, tout, bps = 10000) => [0, pool, tin, tout, bps, coder.encode(['uint32'], [2500])];
const stStep = (pool, tin, tout, i, j, bnbIn) => [11, pool, tin, tout, 10000, coder.encode(['uint256', 'uint256', 'bool'], [i, j, bnbIn])];
const nat = (wn, wrapIn, tin, amt, tout, min, steps) => X.encodeFunctionData('executeNative', [wn, wrapIn, tin, amt, tout, min, FAR, steps]);
const settled = (r, ex) => {
  const t = X.getEvent('NativeSettled').topicHash;
  const l = r.logs.find((x) => x.address.toLowerCase() === ex.toLowerCase() && x.topics[0] === t);
  return l ? coder.decode(['uint256', 'uint256'], l.data).map(BigInt) : null;
};
const bal = (c, t, a) => view(c, t, ERC20.abi, 'balanceOf', [a]).then(BigInt);
/// Выход пары Pancake V2 (комиссия 25/10000 — PancakePair.sol L180-L181).
async function v2Out(c, pair, tin, amt) {
  const res = await L.call(c, LP, pair, new Interface(PAIR.abi).encodeFunctionData('getReserves'));
  const [a, b] = new Interface(PAIR.abi).decodeFunctionResult('getReserves', res.ret).map(BigInt);
  const t0 = await view(c, pair, PAIR.abi, 'token0');
  const [ri, ro] = t0.toLowerCase() === tin.toLowerCase() ? [a, b] : [b, a];
  const inFee = amt * 9975n;
  return (inFee * ro) / (ri * 10000n + inFee);
}

// 2) Продажа BNB.
{
  const { c, w, usdt, pair, ex } = await world();
  const exp = await v2Out(c, pair, w, E18);
  const before = await bal(c, usdt, OWNER);
  const r = await L.call(c, OWNER, ex, nat(w, true, w, E18, usdt, exp, [v2Step(pair, w, usdt)]), E18);
  check(r.ok, `продажа BNB: прошла (газ ${r.gas})`);
  check((await bal(c, usdt, OWNER)) - before === exp, 'продажа BNB: USDT владельцу = getAmountOut пары');
  const s = settled(r, ex);
  check(s && s[0] === E18 && s[1] === 0n, 'продажа BNB: NativeSettled(1 BNB, 0)');
  check((await L.balance(c, ex)) === 0n && (await bal(c, w, ex)) === 0n, 'продажа BNB: на исполнителе нет ни BNB, ни WBNB');
}
// 3) Продажа с остатком.
{
  const { c, w, usdt, pair, ex } = await world();
  const nb = await L.balance(c, OWNER);
  const r = await L.call(c, OWNER, ex, nat(w, true, w, E18, usdt, 1n, [v2Step(pair, w, usdt, 5000)]), E18);
  const s = settled(r, ex);
  check(r.ok && s[0] === E18 && s[1] === E18 / 2n, 'продажа доли 5000 bps: NativeSettled(1, 0,5) — остаток развёрнут');
  check(nb - (await L.balance(c, OWNER)) === E18 / 2n, 'продажа доли: владелец потратил ровно 0,5 BNB (остаток вернулся нативным)');
  check((await bal(c, w, OWNER)) === 0n, 'продажа доли: WBNB на кошельке не осталось');
}
// 4) Покупка BNB + 5) minOut.
{
  const { c, w, usdt, pair, ex } = await world();
  await must(c, OWNER, usdt, ERC20.abi, 'approve', [ex, 10n ** 30n]);
  const amt = 1000n * E18;
  const exp = await v2Out(c, pair, usdt, amt);
  const tooMuch = await L.call(c, OWNER, ex, nat(w, false, usdt, amt, w, exp + 1n, [v2Step(pair, usdt, w)]));
  check(!tooMuch.ok && tooMuch.reason === 'minOut', 'покупка BNB: minOut + 1 wei — откат «minOut»');
  const nb = await L.balance(c, OWNER);
  const ub = await bal(c, usdt, OWNER);
  const r = await L.call(c, OWNER, ex, nat(w, false, usdt, amt, w, exp, [v2Step(pair, usdt, w)]));
  check(r.ok, `покупка BNB: прошла (газ ${r.gas})`);
  check((await L.balance(c, OWNER)) - nb === exp, 'покупка BNB: нативный BNB владельцу = getAmountOut пары');
  check(ub - (await bal(c, usdt, OWNER)) === amt, 'покупка BNB: списано ровно amountIn USDT');
  const s = settled(r, ex);
  check(s && s[0] === 0n && s[1] === exp, 'покупка BNB: NativeSettled(0, получено)');
  check((await bal(c, w, OWNER)) === 0n && (await bal(c, w, ex)) === 0n, 'покупка BNB: WBNB нигде не остался');
  const e2 = await v2Out(c, pair, w, E18);
  const s2 = await L.call(c, OWNER, ex, nat(w, true, w, E18, usdt, e2 + 1n, [v2Step(pair, w, usdt)]), E18);
  check(!s2.ok && s2.reason === 'minOut', 'продажа BNB: minOut + 1 — откат «minOut»');
}
// 6) Неверный wnative, 7) не владелец, 10) BNB напрямую.
{
  const { c, w, usdt, pair, ex } = await world();
  const noCode = '0x' + 'dd'.repeat(20);
  const a = await L.call(c, OWNER, ex, nat(noCode, true, noCode, E18, usdt, 1n, [v2Step(pair, noCode, usdt)]), E18);
  check(!a.ok && a.reason === 'token code', 'wnative без кода — откат «token code»');
  const b = await L.call(c, OWNER, ex, nat(usdt, true, usdt, E18, w, 1n, [v2Step(pair, usdt, w)]), E18);
  check(!b.ok, `wnative = ERC-20 без deposit — откат (${b.reason})`);
  const d = await L.call(c, OWNER, ex, nat(w, true, w, E18, usdt, 1n, [v2Step(pair, w, usdt)]), E18 - 1n);
  check(!d.ok && d.reason === 'wrap', 'value ≠ amountIn — откат «wrap»');
  const e = await L.call(c, OTHER, ex, nat(w, true, w, E18, usdt, 1n, [v2Step(pair, w, usdt)]), E18);
  check(!e.ok && e.reason === 'owner', 'не владелец — откат «owner»');
  const f = await L.call(c, OTHER, ex, '0x', E18);
  check(!f.ok && f.reason === 'bnb', 'BNB напрямую на исполнитель — откат «bnb»');
  const g = await L.call(c, OWNER, ex, nat(w, false, usdt, E18, w, 1n, [v2Step(pair, usdt, w)]), 1n);
  check(!g.ok && g.reason === 'unwrap', 'покупка с value — откат «unwrap»');
}
// 8) Повторный вход из токена.
{
  const { c, w, ex } = await world();
  const t = await L.deploy(c, OWNER, RE, ['uint256'], [10n ** 30n]);
  const f = await L.deploy(c, LP, FACT, ['address'], [LP]);
  await must(c, LP, f, FACT.abi, 'createPair', [t, w]);
  const pair = await view(c, f, FACT.abi, 'getPair', [t, w]);
  await must(c, OWNER, t, RE.abi, 'transfer', [pair, 1000n * E18]);
  await must(c, LP, w, WETH.abi, 'deposit', [], 10n * E18);
  await must(c, LP, w, WETH.abi, 'transfer', [pair, 10n * E18]);
  await must(c, LP, pair, PAIR.abi, 'mint', [LP]);
  await must(c, OWNER, t, RE.abi, 'approve', [ex, 10n ** 30n]);
  await must(c, OWNER, t, RE.abi, 'arm', [ex, nat(w, false, t, 1n, w, 1n, [])]);
  const r = await L.call(c, OWNER, ex, nat(w, false, t, E18, w, 1n, [v2Step(pair, t, w)]));
  check(!r.ok && r.reason === 'token call',
    'повторный вход из токена (вызов исполнителя не владельцем) — откат всей транзакции («token call»)');
  await must(c, OWNER, t, RE.abi, 'arm', ['0x' + '00'.repeat(20), '0x']);
  const ok = await L.call(c, OWNER, ex, nat(w, false, t, E18, w, 1n, [v2Step(pair, t, w)]));
  check(ok.ok, 'тот же маршрут без повторного входа — проходит (причина отката выше — именно повторный вход)');
}
// 9) STABLE_U256_BNB внутри, bnb_gas 2300.
{
  const { c, w, coin, stable, ex } = await world();
  const before = await bal(c, coin, OWNER);
  const r = await L.call(c, OWNER, ex, nat(w, true, w, E18, coin, E18, [stStep(stable, w, coin, 0, 1, true)]), E18);
  const s = settled(r, ex);
  check(r.ok && (await bal(c, coin, OWNER)) - before === E18 && s[0] === E18 && s[1] === 0n,
    'продажа BNB через STABLE_U256_BNB (BNB на входе пула): COIN владельцу, NativeSettled(1, 0)');
  await must(c, OWNER, coin, ERC20.abi, 'approve', [ex, 10n ** 30n]);
  const nb = await L.balance(c, OWNER);
  const b = await L.call(c, OWNER, ex, nat(w, false, coin, E18, w, E18, [stStep(stable, coin, w, 1, 0, false)]));
  const sb = settled(b, ex);
  check(b.ok && (await L.balance(c, OWNER)) - nb === E18 && sb[1] === E18,
    'покупка BNB через STABLE_U256_BNB (BNB от пула с газом 2300 + стипендия): BNB владельцу, NativeSettled(0, 1)');
}
// 11) execute v2 и 12) v1.
for (const [name, art] of [['v2', v2], ['v1', v1]]) {
  const { c, w, usdt, pair, ex } = await world(art);
  await must(c, OWNER, usdt, ERC20.abi, 'approve', [ex, 10n ** 30n]);
  const exp = await v2Out(c, pair, usdt, 1000n * E18);
  const data = X.encodeFunctionData('execute', [usdt, 1000n * E18, w, exp, FAR, [v2Step(pair, usdt, w)]]);
  const r = await L.call(c, OWNER, ex, data);
  check(r.ok && (await bal(c, w, OWNER)) === exp, `${name}: execute USDT → WBNB (ERC-20) — как раньше`);
  const n = await L.call(c, OWNER, ex, nat(w, true, w, E18, usdt, 1n, [v2Step(pair, w, usdt)]), E18);
  check(name === 'v2' ? n.ok : !n.ok, `${name}: executeNative ${name === 'v2' ? 'есть' : 'нет (откат)'}`);
}
console.log('ВСЕ ПРОВЕРКИ ПРОШЛИ');
