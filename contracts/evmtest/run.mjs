// Файл: contracts/evmtest/run.mjs
// Что делает: EVM-тест исполнителя ArbExecutor v2 (решение 272) на EVM ethereumjs (Cancun): реальные PancakePair /
//   PancakeFactory / ERC20 (pancake-swap-core@3b214306) и WETH9 (v2-periphery@ed249913) — обёртка с withdraw через
//   transfer (стипендия 2300 газа); фикстуры только там, где официального кода в репозитории нет (fixtures/, окружение —
//   world.mjs). Здесь проверяется:
//   1) код из contracts/out/ — ровно результат компиляции contracts/ArbExecutor.sol (те же настройки, что build.mjs:
//      решение 457 — via-IR, W.execOpt);
//   2) продажа BNB executeNative(wrapIn = true): value → deposit → пара V2 → USDT владельцу; NativeSettled(value, 0);
//   3) продажа с недоеденным остатком (доля 5000 bps): остаток развёрнут и вернулся нативным BNB, NativeSettled(value, остаток);
//   4) покупка BNB executeNative(wrapIn = false): USDT → пара (прямой вход) → WBNB исполнителю → withdraw → BNB владельцу;
//   5) откат по minOut в обе стороны; 6) обёртка из конструктора (ревью А1): без кода — развёртывание откатывается,
//   ERC-20 без deposit — откат, wnative() = переданный адрес; 7) не владелец; 8) повторный вход из токена;
//   9) шаг STABLE_U256_BNB внутри обоих направлений, bnb_gas пула 2300; 10) BNB напрямую на исполнитель — откат «bnb»;
//   11) execute (ERC-20) v2 — как раньше; 12) исполнитель v1 (contracts/out/v1/) — execute работает, executeNative нет.
//   Сценарии ревью 272 (А4: замок locked, обратные вызовы, злой пул V3, поддельная обёртка, подкинутый BNB, правило
//   «native step», многошаговые маршруты и доли, V3, deadline) — review.mjs, запускаются отсюда. Шаг NOMI_STABLE
//   (решение 284: пара-фикстура Nomiswap Stable, v1 — «kind») — nomi.mjs; шаг METRIC (решение 285) — metric.mjs; шаг IZI (решение 286) — izi.mjs; шаг KYBER_DMM (решение 287) — dmm.mjs; шаг SMARDEX (решение 288) — smardex.mjs; шаг BALANCER (решение 289) — balancer.mjs; шаг LB (решение 290) — lb.mjs;
//   шаги UNI_V4 / INF_CL / INF_BIN исполнителя v3 (решение 395, t181: официальные PoolManager Uniswap V4 и Vault / CL / Bin
//   PancakeSwap Infinity, v2 и v1 — «kind») — singleton.mjs; тоже отсюда. С решения 395 `v2` в проверках — текущая сборка
//   contracts/out/ (исполнитель v3, надмножество v2); замороженная v2 — contracts/out/v2/ (сверка с ней — singleton.mjs).
//   Решение 458 (T457-01): шаги исполнителя v4 с ОФИЦИАЛЬНЫМ кодом протоколов — FLUID (fluid_cases.mjs, Fluid DEX T1 из
//   артефактов развёртывания Arbitrum), BALANCER_V3 (balancerv3.mjs, Vault V3 из balancer-deployments), EKUBO
//   (ekubo_cases.mjs, Core / MEVCapture / Positions из broadcast 42161; T458-02 — пул TWAMM с ордером Orders, ekubo_twamm.mjs); LB20 — нет (solc 0.8.10 нет в package-lock,
//   артефактов v2.0.0 нет — docs/TASKS.md T457-01). Записи — tests/data/executor_v4_evm_2026-11-20/v4_steps.json.
// Кто вызывает: человек или агент: `cd contracts/evmtest && npm ci && node run.mjs` (README.md папки).
// От чего зависит: lib.mjs, world.mjs, review.mjs, contracts/*, vendor/*, fixtures/*.
// Настройки: нет.

import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import * as L from './lib.mjs';
import * as W from './world.mjs';
import { review } from './review.mjs';
import { nomi } from './nomi.mjs';
import { metric } from './metric.mjs';
import { izi } from './izi.mjs';
import { dmm } from './dmm.mjs';
import { smardex } from './smardex.mjs';
import { balancer } from './balancer.mjs';
import { lb } from './lb.mjs';
import { singleton } from './singleton.mjs';
import { fluid } from './fluid_cases.mjs';
import { balancerv3 } from './balancerv3.mjs';
import { ekubo } from './ekubo_cases.mjs';
import { ekuboTwamm } from './ekubo_twamm.mjs';

const { check, must, view } = L;
const { E18, OWNER, LP, OTHER, ERC20, FACT, PAIR, WETH, RE, v2, v1, world, v2Step, stStep, nat, exe, settled, bal, v2Out } = W;
// Запись для тестов бота (tests/data/executor_v2_evm_2026-11-14/): события executeNative настоящего байткода v2.
const record = { note: 'EVM ethereumjs Cancun, contracts/out/ArbExecutor.bin-runtime v2 — contracts/evmtest/run.mjs', cases: {} };

// 1) Сборка и сверка с out/.
const built = L.compile(L.solc08, join(L.here, '..'), 'ArbExecutor.sol', 'ArbExecutor', W.execOpt);
check(built.runtime === v2.runtime && built.bin === v2.bin, 'contracts/out/ArbExecutor.* = компиляция ArbExecutor.sol');
console.log(`размер кода в сети текущей сборки (v4, via-IR): ${v2.runtime.length / 2} байт (EIP-170: 24576, запас ${24576 - v2.runtime.length / 2})`);

// 2) Продажа BNB.
{
  const { c, w, usdt, pair, ex } = await world();
  const exp = await v2Out(c, pair, w, E18);
  const before = await bal(c, usdt, OWNER);
  const r = await L.call(c, OWNER, ex, nat(true, w, E18, usdt, exp, [v2Step(pair, w, usdt)]), E18);
  check(r.ok, `продажа BNB: прошла (газ ${r.gas})`);
  check((await bal(c, usdt, OWNER)) - before === exp, 'продажа BNB: USDT владельцу = getAmountOut пары');
  const s = settled(r, ex);
  check(s && s[0] === E18 && s[1] === 0n, 'продажа BNB: NativeSettled(1 BNB, 0)');
  record.cases.sell = { executor: ex, owner: OWNER, wbnb: w, usdt, value: E18.toString(), usdt_got: exp.toString(), logs: r.logs };
  check((await L.balance(c, ex)) === 0n && (await bal(c, w, ex)) === 0n, 'продажа BNB: на исполнителе нет ни BNB, ни WBNB');
}
// 3) Продажа с остатком.
{
  const { c, w, usdt, pair, ex } = await world();
  const nb = await L.balance(c, OWNER);
  const r = await L.call(c, OWNER, ex, nat(true, w, E18, usdt, 1n, [v2Step(pair, w, usdt, 5000)]), E18);
  const s = settled(r, ex);
  check(r.ok && s[0] === E18 && s[1] === E18 / 2n, 'продажа доли 5000 bps: NativeSettled(1, 0,5) — остаток развёрнут');
  record.cases.sell_left = { executor: ex, owner: OWNER, wbnb: w, usdt, value: E18.toString(), returned: (E18 / 2n).toString(), logs: r.logs };
  check(nb - (await L.balance(c, OWNER)) === E18 / 2n, 'продажа доли: владелец потратил ровно 0,5 BNB (остаток вернулся нативным)');
  check((await bal(c, w, OWNER)) === 0n, 'продажа доли: WBNB на кошельке не осталось');
}
// 4) Покупка BNB + 5) minOut.
{
  const { c, w, usdt, pair, ex } = await world();
  await must(c, OWNER, usdt, ERC20.abi, 'approve', [ex, 10n ** 30n]);
  const amt = 1000n * E18;
  const exp = await v2Out(c, pair, usdt, amt);
  const tooMuch = await L.call(c, OWNER, ex, nat(false, usdt, amt, w, exp + 1n, [v2Step(pair, usdt, w)]));
  check(!tooMuch.ok && tooMuch.reason === 'minOut', 'покупка BNB: minOut + 1 wei — откат «minOut»');
  const nb = await L.balance(c, OWNER);
  const ub = await bal(c, usdt, OWNER);
  const r = await L.call(c, OWNER, ex, nat(false, usdt, amt, w, exp, [v2Step(pair, usdt, w)]));
  check(r.ok, `покупка BNB: прошла (газ ${r.gas})`);
  check((await L.balance(c, OWNER)) - nb === exp, 'покупка BNB: нативный BNB владельцу = getAmountOut пары');
  check(ub - (await bal(c, usdt, OWNER)) === amt, 'покупка BNB: списано ровно amountIn USDT');
  const s = settled(r, ex);
  check(s && s[0] === 0n && s[1] === exp, 'покупка BNB: NativeSettled(0, получено)');
  record.cases.buy = { executor: ex, owner: OWNER, wbnb: w, usdt, usdt_spent: amt.toString(), bnb_got: exp.toString(), logs: r.logs };
  check((await bal(c, w, OWNER)) === 0n && (await bal(c, w, ex)) === 0n, 'покупка BNB: WBNB нигде не остался');
  const e2 = await v2Out(c, pair, w, E18);
  const s2 = await L.call(c, OWNER, ex, nat(true, w, E18, usdt, e2 + 1n, [v2Step(pair, w, usdt)]), E18);
  check(!s2.ok && s2.reason === 'minOut', 'продажа BNB: minOut + 1 — откат «minOut»');
}
// 6) Обёртка из конструктора, 7) не владелец, 10) BNB напрямую.
{
  const { c, w, usdt, pair, ex } = await world();
  const noCode = '0x' + 'dd'.repeat(20);
  const a = await L.call(c, OWNER, null, '0x' + v2.bin + L.coder.encode(['address'], [noCode]).slice(2));
  check(!a.ok && a.reason === 'wnative', 'развёртывание с обёрткой без кода — откат «wnative»');
  check((await view(c, ex, v2.abi, 'wnative')).toLowerCase() === w.toLowerCase(), 'wnative() = адрес из конструктора');
  const exU = await W.executor(c, OWNER, v2, usdt);
  const b = await L.call(c, OWNER, exU, nat(true, usdt, E18, w, 1n, [v2Step(pair, usdt, w)]), E18);
  check(!b.ok, `обёртка = ERC-20 без deposit — откат (${b.reason})`);
  const b2 = await L.call(c, OWNER, ex, nat(true, usdt, E18, w, 1n, [v2Step(pair, usdt, w)]), E18);
  check(!b2.ok && b2.reason === 'wrap', 'продажа с tokenIn ≠ обёртке исполнителя — откат «wrap»');
  const d = await L.call(c, OWNER, ex, nat(true, w, E18, usdt, 1n, [v2Step(pair, w, usdt)]), E18 - 1n);
  check(!d.ok && d.reason === 'wrap', 'value ≠ amountIn — откат «wrap»');
  const e = await L.call(c, OTHER, ex, nat(true, w, E18, usdt, 1n, [v2Step(pair, w, usdt)]), E18);
  check(!e.ok && e.reason === 'owner', 'не владелец — откат «owner»');
  const f = await L.call(c, OTHER, ex, '0x', E18);
  check(!f.ok && f.reason === 'bnb', 'BNB напрямую на исполнитель — откат «bnb»');
  const g = await L.call(c, OWNER, ex, nat(false, usdt, E18, w, 1n, [v2Step(pair, usdt, w)]), 1n);
  check(!g.ok && g.reason === 'unwrap', 'покупка с value — откат «unwrap»');
}
// 8) Повторный вход из токена (вызов не владельцем — откат; замок locked — review.mjs).
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
  await must(c, OWNER, t, RE.abi, 'arm', [ex, nat(false, t, 1n, w, 1n, [])]);
  const r = await L.call(c, OWNER, ex, nat(false, t, E18, w, 1n, [v2Step(pair, t, w)]));
  check(!r.ok && r.reason === 'token call',
    'повторный вход из токена (вызов исполнителя не владельцем) — откат всей транзакции («token call»)');
  await must(c, OWNER, t, RE.abi, 'arm', ['0x' + '00'.repeat(20), '0x']);
  const ok = await L.call(c, OWNER, ex, nat(false, t, E18, w, 1n, [v2Step(pair, t, w)]));
  check(ok.ok, 'тот же маршрут без повторного входа — проходит (причина отката выше — именно повторный вход)');
}
// 9) STABLE_U256_BNB внутри, bnb_gas 2300.
{
  const { c, w, coin, stable, ex } = await world();
  const before = await bal(c, coin, OWNER);
  const r = await L.call(c, OWNER, ex, nat(true, w, E18, coin, E18, [stStep(stable, w, coin, 0, 1, true)]), E18);
  const s = settled(r, ex);
  check(r.ok && (await bal(c, coin, OWNER)) - before === E18 && s[0] === E18 && s[1] === 0n,
    'продажа BNB через STABLE_U256_BNB (BNB на входе пула): COIN владельцу, NativeSettled(1, 0)');
  await must(c, OWNER, coin, ERC20.abi, 'approve', [ex, 10n ** 30n]);
  const nb = await L.balance(c, OWNER);
  const b = await L.call(c, OWNER, ex, nat(false, coin, E18, w, E18, [stStep(stable, coin, w, 1, 0, false)]));
  const sb = settled(b, ex);
  check(b.ok && (await L.balance(c, OWNER)) - nb === E18 && sb[1] === E18,
    'покупка BNB через STABLE_U256_BNB (BNB от пула с газом 2300 + стипендия): BNB владельцу, NativeSettled(0, 1)');
}
// 11) execute v2 и 12) v1.
for (const [name, art] of [['v2', v2], ['v1', v1]]) {
  const { c, w, usdt, pair, ex } = await world(art);
  await must(c, OWNER, usdt, ERC20.abi, 'approve', [ex, 10n ** 30n]);
  const exp = await v2Out(c, pair, usdt, 1000n * E18);
  const r = await L.call(c, OWNER, ex, exe(usdt, 1000n * E18, w, exp, [v2Step(pair, usdt, w)]));
  check(r.ok && (await bal(c, w, OWNER)) === exp, `${name}: execute USDT → WBNB (ERC-20) — как раньше`);
  const n = await L.call(c, OWNER, ex, nat(true, w, E18, usdt, 1n, [v2Step(pair, w, usdt)]), E18);
  check(name === 'v2' ? n.ok : !n.ok, `${name}: executeNative ${name === 'v2' ? 'есть' : 'нет (откат)'}`);
}
await review(record);
await nomi();
await metric();
await izi();
await dmm();
await smardex();
await balancer();
await lb();
const steps = await singleton();
// Решение 458 (T457-01): шаги FLUID / BALANCER_V3 / EKUBO исполнителя v4 с официальным кодом протоколов.
const v4steps = { fluid: await fluid(), balancer_v3: await balancerv3(), ekubo: await ekubo(), ekubo_twamm: await ekuboTwamm() };
// Решение 457: запись текущей сборки (v4) — в свою папку; запись v3 (executor_v3_evm_2026-11-17) не перезаписывается.
const out3 = join(L.here, '..', '..', 'tests', 'data', 'executor_v4_evm_2026-11-20');
mkdirSync(out3, { recursive: true });
writeFileSync(join(out3, 'singleton_steps.json'), JSON.stringify(steps, null, 2) + '\n');
writeFileSync(join(out3, 'v4_steps.json'), JSON.stringify(v4steps, null, 2) + '\n');
const out = join(L.here, '..', '..', 'tests', 'data', 'executor_v2_evm_2026-11-14');
mkdirSync(out, { recursive: true });
writeFileSync(join(out, 'native_settled.json'), JSON.stringify(record, null, 2) + '\n');
console.log('ВСЕ ПРОВЕРКИ ПРОШЛИ');
