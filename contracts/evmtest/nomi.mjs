// Файл: contracts/evmtest/nomi.mjs
// Что делает: решение 284 — шаг NOMI_STABLE (вид 12) ArbExecutor на EVM ethereumjs (Cancun) с парой-фикстурой
//   fixtures/NomiStableFixture.sol (строки обмена NomiswapStablePair) и официальными PancakePair / ERC20:
//   N1) v2: execute USDT → COIN одним шагом 12 — прямой вход (transferFrom владелец → пара), владелец получил ровно
//       getAmountOut(tokenIn, вход) пары, minOut + 1 — откат «minOut»;
//   N2) v2: шаг 12 вторым шагом (вход — баланс исполнителя): цикл USDT →(PancakePair)→ COIN →(пара 12)→ USDT;
//   N3) тот же маршрут шагом SOLIDLY (вид 3) — откат: у пары нет getAmountOut(uint256,address);
//   N4) исполнитель v1 (contracts/out/v1/) — шаг 12 откатывается «kind» (вида нет; бот у кошелька v1 шаг не строит —
//       pools/exec_callback.rs step_missing).
//   Проба на настоящей паре на цепи — tests/data/nomiswap_stable_2026-11-15/exec_sim.json.
// Кто вызывает: contracts/evmtest/run.mjs.
// От чего зависит: lib.mjs, world.mjs, fixtures/NomiStableFixture.sol.
// Настройки: нет.

import { join } from 'node:path';
import * as L from './lib.mjs';
import * as W from './world.mjs';

const { check, must, view } = L;
const { E18, OWNER, LP, ERC20, v1, v2, world, v2Step, exe, bal, v2Out } = W;
const NOMI = L.compile(L.solc08, join(L.here, 'fixtures'), 'NomiStableFixture.sol', 'NomiStableFixture', W.opt);
const FEE = 6n; // swapFee пары USDT/USDC на блоке 133650877 (probe.json) — над MAX_FEE = 100000.
const step = (kind, pool, tin, tout) => [kind, pool, tin, tout, 10000, '0x'];

/// Пара-фикстура USDT/COIN с ликвидностью 50 000 : 50 000 от LP. Возвращает адрес.
async function nomiPair(c, usdt, coin) {
  const p = await L.deploy(c, LP, NOMI, ['address', 'address', 'uint32'], [usdt, coin, FEE]);
  await must(c, LP, usdt, ERC20.abi, 'transfer', [p, 50_000n * E18]);
  await must(c, LP, coin, ERC20.abi, 'transfer', [p, 50_000n * E18]);
  await must(c, LP, p, NOMI.abi, 'sync', []);
  return p;
}

/// Сценарии N1–N4. Ошибок нет: провал — check бросает.
export async function nomi() {
  {
    const { c, usdt, coin, ex } = await world(v2);
    const p = await nomiPair(c, usdt, coin);
    await must(c, OWNER, usdt, ERC20.abi, 'approve', [ex, 10n ** 30n]);
    const amt = 1000n * E18;
    const exp = BigInt(await view(c, p, NOMI.abi, 'getAmountOut', [usdt, amt]));
    const bad = await L.call(c, OWNER, ex, exe(usdt, amt, coin, exp + 1n, [step(12, p, usdt, coin)]));
    check(!bad.ok && bad.reason === 'minOut', 'NOMI_STABLE: minOut + 1 — откат «minOut»');
    const before = await bal(c, coin, OWNER);
    const r = await L.call(c, OWNER, ex, exe(usdt, amt, coin, exp, [step(12, p, usdt, coin)]));
    check(r.ok, `NOMI_STABLE: execute USDT → COIN прошёл (газ ${r.gas})`);
    check((await bal(c, coin, OWNER)) - before === exp, 'NOMI_STABLE: владелец получил ровно getAmountOut(tokenIn, вход) пары');
    check((await bal(c, usdt, ex)) === 0n && (await bal(c, coin, ex)) === 0n, 'NOMI_STABLE: на исполнителе ничего не осталось');
    const s = await L.call(c, OWNER, ex, exe(usdt, amt, coin, 1n, [step(3, p, usdt, coin)]));
    check(!s.ok, 'тот же маршрут шагом SOLIDLY (вид 3) — откат: у пары нет getAmountOut(uint256,address)');
  }
  {
    const { c, usdt, coin, pairCU, ex } = await world(v2);
    const p = await nomiPair(c, usdt, coin);
    await must(c, OWNER, usdt, ERC20.abi, 'approve', [ex, 10n ** 30n]);
    const amt = 100n * E18;
    const mid = await v2Out(c, pairCU, usdt, amt);
    const exp = BigInt(await view(c, p, NOMI.abi, 'getAmountOut', [coin, mid]));
    const before = await bal(c, usdt, OWNER);
    const route = [v2Step(pairCU, usdt, coin), step(12, p, coin, usdt)];
    const r = await L.call(c, OWNER, ex, exe(usdt, amt, usdt, 1n, route));
    check(r.ok, `NOMI_STABLE вторым шагом (вход — баланс исполнителя): цикл прошёл (газ ${r.gas})`);
    check((await bal(c, usdt, OWNER)) - (before - amt) === exp, 'NOMI_STABLE вторым шагом: выход = getAmountOut пары на выход шага V2');
  }
  {
    const { c, usdt, coin, ex } = await world(v1);
    const p = await nomiPair(c, usdt, coin);
    await must(c, OWNER, usdt, ERC20.abi, 'approve', [ex, 10n ** 30n]);
    const r = await L.call(c, OWNER, ex, exe(usdt, E18, coin, 1n, [step(12, p, usdt, coin)]));
    check(!r.ok && r.reason === 'kind', 'исполнитель v1: шаг NOMI_STABLE (12) — откат «kind» (вида нет в v1)');
  }
}
